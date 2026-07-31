#![allow(dead_code)]

use std::{
    collections::HashMap,
    env,
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::{Arc, OnceLock, RwLock as StdRwLock},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;

use super::{
    dataplane::{ChildSaStateMachine, DataplaneStateError},
    eap_aka::{build_challenge_response, build_sync_failure_response, parse_challenge},
    epdg,
    executor::{
        readiness_key_for_stage, soak_observation_for_stage, ExecutorStage, ExecutorStageRequest,
        ExecutorStageResult, ExecutorStageStatus, LiveExecutorGateReport,
    },
    ike_codec::IkeExchangeType,
    ike_dh::{DhGroup, Modp2048Ephemeral},
    ike_encrypted::encrypted_response_header_matches,
    ike_identity::{build_permanent_nai, IkeIdentityError},
    ike_keys::{ChildSaKeySchedulePlan, ChildSaSecretPair},
    ike_payloads::ike_proposal_dh_group_from_profile_string,
    ike_state::{IkeAuthProgress, IkeConfigurationMaterial, IkeStateMachine},
    ims,
    profiles::{self, CarrierProfile},
    qmi_uim::{
        execute_usim_authenticate_via_proxy_reason_with_retry,
        verify_usim_application_via_proxy_reason_with_retry, USIM_AID_PREFIX,
    },
    sms,
    transport::{ResolvedEpdgEndpoint, TransportError, UdpSocketDatagramTransport},
    tun_gateway::{
        self, ImsEspFlowConfig, ImsEspPolicyConfig, TunGatewayConfig, TunGatewayRuntime,
    },
    voice,
};
use crate::hardware::cellular::modem_manager::{current_sim_identity, get_sim_info_data_with_cache};
use crate::platform::config::{LineVowifiConfig, VowifiProxyMode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, TcpStream},
    sync::{mpsc, Mutex},
};
use tracing::{debug, error, info, warn};

const LIVE_DNS_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_IKE_SA_INIT_TIMEOUT: Duration = Duration::from_secs(4);
const LIVE_IKE_AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const LIVE_SIM_AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const LIVE_SIM_AUTH_ATTEMPTS: usize = 3;
const LIVE_SIM_AUTH_RETRY_DELAY: Duration = Duration::from_millis(250);
const LIVE_SIM_AUTH_GATE_TIMEOUT: Duration = Duration::from_secs(3);
const LIVE_SIM_AUTH_GATE_ATTEMPTS: usize = 4;
const LIVE_SIM_AUTH_GATE_RETRY_DELAY: Duration = Duration::from_millis(500);
const LIVE_IKE_NONCE_BYTES: usize = 32;
const LIVE_IKE_SA_INIT_ATTEMPTS: usize = 1;
const LIVE_IKE_AUTH_ATTEMPTS: usize = 3;
const LIVE_IKE_MAX_ENDPOINTS_PER_PASS: usize = 5;
const LIVE_IKE_MAX_PROPOSAL_GROUPS_PER_PASS: usize = 2;
const LIVE_IKE_MAX_TRANSPORT_PATHS_PER_PASS: usize = 2;
const IKE_PORT: u16 = 500;
const IKE_NAT_T_PORT: u16 = 4500;
const DEFAULT_QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const DEFAULT_QMI_DEVICE: &str = "/dev/wwan0qmi0";
const DEFAULT_UIM_SLOT: u8 = 1;
const DEFAULT_LIVE_TUN_NAME: &str = "sa_vwf0";
const LIVE_IMS_TCP_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_IMS_REGISTER_READ_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_IMS_REGISTER_DEFAULT_TTL: Duration = Duration::from_secs(300);
const LIVE_IMS_REGISTER_MAX_TTL: Duration = Duration::from_secs(3600);
const LIVE_IMS_REGISTER_EXPIRY_SKEW: Duration = Duration::from_secs(60);
const LIVE_SMS_SEND_TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_SMS_FOLLOWUP_WINDOW: Duration = Duration::from_secs(20);
const LIVE_VOICE_INVITE_TOTAL_TIMEOUT: Duration = Duration::from_secs(32);
const LIVE_VOICE_MMTEL_ICSI: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
const LIVE_IMS_SECURITY_PORT_C: u16 = 5064;
const LIVE_IMS_SECURITY_PORT_S: u16 = 5063;
const IMS_MMTEL_ICSI_REF: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
const ENV_QMI_PROXY_SOCKET: &str = "SIMADMIN_VOWIFI_QMI_PROXY_SOCKET";
const ENV_QMI_DEVICE: &str = "SIMADMIN_VOWIFI_QMI_DEVICE";
const ENV_UIM_SLOT: &str = "SIMADMIN_VOWIFI_UIM_SLOT";
const ENV_TUN_NAME: &str = "SIMADMIN_VOWIFI_TUN_NAME";
const ENV_IMS_SECURITY_PORT_C: &str = "SIMADMIN_VOWIFI_IMS_SECURITY_PORT_C";
const ENV_IMS_SECURITY_PORT_S: &str = "SIMADMIN_VOWIFI_IMS_SECURITY_PORT_S";

/// Live TUN gateways, keyed by `line_id`.
///
/// One entry per line: each VoWiFi tunnel owns its own TUN device holding the
/// inner address that line's carrier assigned, so several SIMs can be connected
/// at once. A single shared slot would let the second line evict the first one's
/// gateway while its tunnel was still in use.
static LIVE_TUN_GATEWAY: OnceLock<Mutex<HashMap<String, Arc<TunGatewayRuntime>>>> = OnceLock::new();
// The four IMS session caches below are keyed by `line_id`.
//
// They were single slots, which broke on a multi-SIM host: two lines on the same
// carrier profile shared one entry (and one TTL), and two lines on different
// profiles evicted each other. `LIVE_IMS_TCP_CHANNEL` was the most damaging —
// it holds a live TCP socket, so line B's REGISTER could reuse or close line A's
// connection. The `profile_id` check inside each entry is retained as a staleness
// guard for when a line switches carriers.
static LIVE_IMS_REGISTER_READY: OnceLock<Mutex<HashMap<String, LiveImsRegisterReady>>> =
    OnceLock::new();
static LIVE_IMS_SECURITY_VERIFY: OnceLock<Mutex<HashMap<String, LiveImsSecurityVerify>>> =
    OnceLock::new();
static LIVE_IMS_TCP_CHANNEL: OnceLock<Mutex<HashMap<String, LiveImsTcpChannel>>> = OnceLock::new();
static LIVE_IMS_REGISTER_SUCCESS_VARIANT: OnceLock<
    Mutex<HashMap<String, LiveImsRegisterSuccessVariant>>,
> = OnceLock::new();
/// Per-line network overrides, keyed by `line_id`.
///
/// Deliberately a map rather than a single value: each SIM may sit on a different
/// operator in a different country and needs its own ePDG, DNS and proxy. A global
/// override would make configuring line A silently change line B.
static LIVE_NETWORK_OVERRIDES: OnceLock<StdRwLock<HashMap<String, LiveNetworkOverrides>>> =
    OnceLock::new();

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LiveNetworkOverrides {
    /// Pin this line to a specific carrier profile by `profile_id`. `None`
    /// resolves the profile automatically from the SIM's IMSI. A pinned id that
    /// no longer resolves falls back to automatic matching, so deleting a
    /// profile never strands a line.
    profile_id: Option<String>,
    dns_server: Option<IpAddr>,
    /// How this line's IKE/NAT-T traffic egresses. `None` means direct.
    proxy: Option<LiveProxySetting>,
}

/// A validated egress proxy for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveProxySetting {
    Socks5(super::socks5::Socks5Endpoint),
}

fn network_overrides_map() -> &'static StdRwLock<HashMap<String, LiveNetworkOverrides>> {
    LIVE_NETWORK_OVERRIDES.get_or_init(|| StdRwLock::new(HashMap::new()))
}

/// Store (or clear) the overrides for one line.
pub fn configure_live_network_overrides(
    line_id: &str,
    config: &LineVowifiConfig,
) -> Result<(), String> {
    let next = build_live_network_overrides(config)?;
    let mut map = network_overrides_map()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if next == LiveNetworkOverrides::default() {
        map.remove(line_id);
    } else {
        map.insert(line_id.to_string(), next);
    }
    Ok(())
}

/// Drop a line's overrides, e.g. when the line disappears.
pub fn forget_live_network_overrides(line_id: &str) {
    network_overrides_map()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(line_id);
}

fn build_live_network_overrides(config: &LineVowifiConfig) -> Result<LiveNetworkOverrides, String> {
    // Only transports that can actually carry UDP 500/4500 are accepted here.
    let proxy = match config.proxy_mode {
        VowifiProxyMode::Direct => None,
        VowifiProxyMode::Socks5UdpAssociate => Some(LiveProxySetting::Socks5(
            super::socks5::Socks5Endpoint::parse(&config.proxy_endpoint)
                .map_err(|error| error.to_string())?,
        )),
        // Not implemented: a private relay protocol adds no value over pointing
        // this line at a self-hosted standard SOCKS5 server.
        VowifiProxyMode::UdpRelay => {
            return Err("vowifi_proxy_mode_not_implemented:udp_relay".to_string())
        }
    };
    Ok(LiveNetworkOverrides {
        dns_server: if config.dns_server.is_empty() {
            None
        } else {
            Some(
                config
                    .dns_server
                    .parse::<IpAddr>()
                    .map_err(|_| "vowifi_dns_server_invalid".to_string())?,
            )
        },
        proxy,
        profile_id: config
            .profile_id
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

fn line_overrides(line_id: &str) -> LiveNetworkOverrides {
    network_overrides_map()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned()
        .unwrap_or_default()
}

/// The carrier profile a line is pinned to, if any. Empty `line_id`, an unknown
/// line, or a line with no pin all return `None`.
pub fn line_pinned_profile_id(line_id: &str) -> Option<String> {
    if line_id.is_empty() {
        return None;
    }
    line_overrides(line_id).profile_id
}

/// Resolve the carrier profile for a line, honoring a per-line pin.
///
/// A pinned `profile_id` is tried first, and only against database-published
/// profiles; if it no longer resolves (the profile was deleted, or the id was
/// never valid) we fall back to matching by IMSI, so a stale pin degrades to the
/// automatic behaviour instead of stranding the line. With no pin this is
/// exactly `resolve_by_imsi`.
pub fn resolve_profile_for_line(line_id: &str, imsi: &str) -> Option<profiles::CarrierMatch> {
    let pinned = line_pinned_profile_id(line_id);
    profiles::resolve_for_line(pinned.as_deref(), imsi.trim())
}

fn live_epdg_settings(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> (String, u16, Option<IpAddr>) {
    // The ePDG host/port now come solely from the carrier profile; a line can
    // pick which profile via its `profile_id` pin, but no longer hand-overrides
    // the endpoint. DNS remains a per-line override.
    (
        profile.epdg.host.to_string(),
        profile.epdg.port,
        live_dns_candidates(line_id, profile).into_iter().next(),
    )
}

/// DNS servers to try, in order: this line's override first, then the carrier
/// profile's list. Resolving the ePDG FQDN is a hard prerequisite for
/// connecting at all, so a single unreachable resolver must not be fatal.
fn live_dns_candidates(line_id: &str, profile: &'static CarrierProfile) -> Vec<IpAddr> {
    let mut candidates = Vec::new();
    if let Some(server) = line_overrides(line_id).dns_server {
        candidates.push(server);
    }
    for server in profile.epdg.dns_servers {
        if let Some(addr) = super::profile_record::parse_dns_server(server) {
            if !candidates.contains(&addr.ip()) {
                candidates.push(addr.ip());
            }
        }
    }
    if let Some(server) = profile
        .epdg
        .dns_server
        .and_then(|value| super::profile_record::parse_dns_server(value))
    {
        if !candidates.contains(&server.ip()) {
            candidates.push(server.ip());
        }
    }
    candidates
}

async fn resolve_live_epdg(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<ResolvedEpdgEndpoint, TransportError> {
    let overrides = line_overrides(line_id);
    let host = profile.epdg.host.to_string();
    let port = profile.epdg.port;
    // DNS follows the proxy: when this line egresses through a proxy, the lookup
    // goes through it too, so the real client IP is not exposed to the resolver
    // and operator DNS interception on the ePDG name is bypassed. With no proxy
    // configured the query goes out directly to the configured server.
    let proxy = overrides.proxy;
    // An empty candidate list means "no explicit server": a single `None`
    // attempt lets the resolver layer fall back to the system resolver.
    let mut candidates: Vec<Option<IpAddr>> = live_dns_candidates(line_id, profile)
        .into_iter()
        .map(Some)
        .collect();
    if candidates.is_empty() {
        candidates.push(None);
    }
    let mut last_error = None;
    for dns_server in candidates {
        let attempt = match &proxy {
            Some(LiveProxySetting::Socks5(endpoint)) => {
                epdg::resolve_epdg_via_socks5(&profile.meta, &host, port, dns_server, endpoint)
                    .await
            }
            None => {
                epdg::resolve_epdg_with_dns_override(&profile.meta, &host, port, dns_server).await
            }
        };
        match attempt {
            Ok(endpoint) => return Ok(endpoint),
            Err(error) => {
                warn!(
                    line_id = %line_id,
                    dns_server = ?dns_server,
                    error = %error,
                    "ePDG resolution failed on this DNS server; trying the next candidate"
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| TransportError::DnsFailed("no_dns_candidate_available".to_string())))
}

#[derive(Debug, Clone)]
struct LiveImsRegisterReady {
    profile_id: &'static str,
    expires_at: Instant,
    sms_capability_advertised: bool,
    receiver_transport: &'static str,
}

#[derive(Debug, Clone)]
struct LiveImsSecurityVerify {
    profile_id: &'static str,
    expires_at: Instant,
    value: String,
}

struct LiveImsTcpChannel {
    profile_id: &'static str,
    expires_at: Instant,
    channel: super::channel::EpdgSipChannel,
}

#[derive(Debug, Clone)]
struct LiveImsRegisterSuccessVariant {
    profile_id: &'static str,
    label: &'static str,
    captured_at: Instant,
}

#[derive(Debug)]
pub struct LiveSmsSendResult {
    pub outcome: sms::MoSmsSipOutcome,
    pub followup: mpsc::UnboundedReceiver<LiveSmsFollowupFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSmsFollowupFrame {
    pub outcome: sms::MoSmsSipOutcome,
}

/// Synchronous result of placing a live VoWiFi voice call: the INVITE outcome
/// plus a follow-up channel that reports dialog/media progress (ringing,
/// answer, hangup, media counters) as the call proceeds.
#[derive(Debug)]
pub struct LiveCallResult {
    pub outcome: voice::MoCallSipOutcome,
    pub followup: mpsc::UnboundedReceiver<LiveCallFollowupFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCallFollowupFrame {
    pub outcome: voice::MoCallSipOutcome,
}

/// Longest interface name the kernel accepts (`IFNAMSIZ` - 1).
const MAX_IFNAME_LEN: usize = 15;

/// Derive this line's TUN device name from the configured base name.
///
/// Each concurrently connected line needs its **own** TUN device: the device
/// holds the inner address that line's carrier assigned, and two lines cannot
/// share one interface. Deriving the name from `line_id` keeps it stable across
/// reconnects (so a restarted tunnel reclaims its own device rather than piling
/// up interfaces) while staying unique per line.
///
/// An empty `line_id` yields the base name unchanged, preserving single-line
/// behaviour and the `SIMADMIN_VOWIFI_TUN_NAME` override.
fn tun_name_for_line(base: &str, line_id: &str) -> String {
    if line_id.is_empty() {
        return base.to_string();
    }
    // `line_id` is `line-<md5>`; a short slice of its hex digits is enough to
    // separate lines while leaving room inside IFNAMSIZ.
    let suffix: String = line_id
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .rev()
        .take(6)
        .map(|byte| (byte as char).to_ascii_lowercase())
        .collect();
    let mut name = String::with_capacity(MAX_IFNAME_LEN);
    let room = MAX_IFNAME_LEN.saturating_sub(suffix.len());
    name.extend(base.chars().take(room));
    name.push_str(&suffix);
    name
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveRuntimeConfig {
    qmi_proxy_socket: String,
    qmi_device: String,
    uim_slot: u8,
    tun_name: String,
    ims_security_port_c: u16,
    ims_security_port_s: u16,
}

impl LiveRuntimeConfig {
    fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    fn from_lookup<F>(mut lookup: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        Self {
            qmi_proxy_socket: read_non_empty_config(
                lookup(ENV_QMI_PROXY_SOCKET),
                DEFAULT_QMI_PROXY_SOCKET,
            ),
            qmi_device: read_non_empty_config(lookup(ENV_QMI_DEVICE), DEFAULT_QMI_DEVICE),
            uim_slot: read_u8_config(lookup(ENV_UIM_SLOT), DEFAULT_UIM_SLOT),
            tun_name: read_non_empty_config(lookup(ENV_TUN_NAME), DEFAULT_LIVE_TUN_NAME),
            ims_security_port_c: read_u16_config(
                lookup(ENV_IMS_SECURITY_PORT_C),
                LIVE_IMS_SECURITY_PORT_C,
            ),
            ims_security_port_s: read_u16_config(
                lookup(ENV_IMS_SECURITY_PORT_S),
                LIVE_IMS_SECURITY_PORT_S,
            ),
        }
    }
}

fn live_runtime_config() -> LiveRuntimeConfig {
    LiveRuntimeConfig::from_env()
}

/// Which SIM/QMI device each line must use, keyed by `line_id`.
///
/// The environment-derived [`LiveRuntimeConfig`] carries a single
/// `qmi_device`/`uim_slot` pair, which is only correct on a one-baseband host. On
/// a multi-SIM device every line has to run SIM authentication and IKE identity
/// against *its own* reader, otherwise line B would authenticate with line A's
/// card. Registering each line's binding here keeps that mapping explicit.
static LIVE_LINE_SIM_DEVICES: OnceLock<StdRwLock<HashMap<String, LiveSimDevice>>> = OnceLock::new();

/// The SIM access parameters for one line.
///
/// `qmi_device`/`uim_slot` address the reader for QMI/UIM operations (EAP-AKA,
/// SIM auth). `modem_path` addresses the same line through ModemManager, which is
/// what identity lookups (IMSI) use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSimDevice {
    pub qmi_device: String,
    pub uim_slot: u8,
    pub modem_path: String,
}

fn line_sim_devices() -> &'static StdRwLock<HashMap<String, LiveSimDevice>> {
    LIVE_LINE_SIM_DEVICES.get_or_init(|| StdRwLock::new(HashMap::new()))
}

/// Record which reader a line owns. Called when lines are discovered/refreshed.
pub fn register_line_sim_device(line_id: &str, qmi_device: &str, uim_slot: u8, modem_path: &str) {
    if line_id.is_empty() || qmi_device.is_empty() {
        return;
    }
    line_sim_devices()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            line_id.to_string(),
            LiveSimDevice {
                qmi_device: qmi_device.to_string(),
                uim_slot,
                modem_path: modem_path.to_string(),
            },
        );
}

/// Resolve this line's SIM identity (IMSI etc.) through ModemManager.
///
/// Uses the line's own `modem_path` so a second line reports its own subscriber
/// rather than the first modem's. Falls back to the global lookup when the line is
/// unknown, preserving single-baseband behaviour.
async fn line_sim_identity(
    line_id: &str,
    conn: &zbus::Connection,
) -> Option<crate::hardware::cellular::modem_manager::SimIdentity> {
    let modem_path = sim_device_for_line(line_id).modem_path;
    if modem_path.is_empty() {
        return current_sim_identity(conn).await;
    }
    crate::hardware::cellular::modem_manager::sim_identity_for_modem(conn, &modem_path).await
}

/// Forget a line's reader mapping (line removed).
pub fn forget_line_sim_device(line_id: &str) {
    line_sim_devices()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(line_id);
}

/// Resolve the SIM device for a line, falling back to the global/env values when
/// the line is unknown (single-baseband hosts and the legacy code paths).
pub(crate) fn sim_device_for_line(line_id: &str) -> LiveSimDevice {
    if !line_id.is_empty() {
        if let Some(device) = line_sim_devices()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(line_id)
            .cloned()
        {
            return device;
        }
    }
    let config = live_runtime_config();
    LiveSimDevice {
        qmi_device: config.qmi_device,
        uim_slot: config.uim_slot,
        // No line context: identity falls back to the global first-modem lookup.
        modem_path: String::new(),
    }
}

/// Verify SIM auth access for a specific line's reader.
pub async fn verify_live_sim_auth_access_for_line(line_id: &str) -> Result<(), LiveStageError> {
    let proxy_socket = live_runtime_config().qmi_proxy_socket;
    let device = sim_device_for_line(line_id);
    tokio::task::spawn_blocking(move || {
        verify_usim_application_via_proxy_reason_with_retry(
            proxy_socket.as_str(),
            device.qmi_device.as_str(),
            device.uim_slot,
            USIM_AID_PREFIX,
            LIVE_SIM_AUTH_GATE_ATTEMPTS,
            LIVE_SIM_AUTH_GATE_TIMEOUT,
            LIVE_SIM_AUTH_GATE_RETRY_DELAY,
        )
    })
    .await
    .map_err(|_| live_stage_error("sim_auth_gate_runtime_failed"))?
    .map_err(live_stage_error)?;
    info!("SIMAuth access gate passed");
    Ok(())
}

pub async fn verify_live_sim_auth_access() -> Result<(), LiveStageError> {
    verify_live_sim_auth_access_for_line("").await
}

fn read_non_empty_config(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn read_u8_config(value: Option<String>, default: u8) -> u8 {
    value
        .and_then(|value| value.trim().parse::<u8>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn read_u16_config(value: Option<String>, default: u16) -> u16 {
    value
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy)]
struct LiveRegisterHeaderVariant {
    label: &'static str,
    force_sec_agree_headers: bool,
    include_route_header: bool,
    include_security_client: bool,
    initial_authorization: LiveInitialAuthorizationFormat,
    security_client_format: LiveSecurityClientFormat,
    request_uri: LiveRegisterRequestUri,
    identity_format: LiveRegisterIdentityFormat,
    header_profile: LiveRegisterHeaderProfile,
}

#[derive(Debug, Clone, Copy)]
enum LiveSecurityClientFormat {
    FullSpaced,
    FullCompact,
    MinimalSpaced,
}

impl LiveSecurityClientFormat {
    fn label(self) -> &'static str {
        match self {
            Self::FullSpaced => "full_spaced",
            Self::FullCompact => "full_compact",
            Self::MinimalSpaced => "minimal_spaced",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveInitialAuthorizationFormat {
    None,
    AkaEmpty,
    AkaEmptyUriFirst,
    AkaEmptyUriFirstNoAlgorithm,
    AkaZeroResponse,
    AkaZeroResponseUriFirst,
}

impl LiveInitialAuthorizationFormat {
    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AkaEmpty => "aka_empty",
            Self::AkaEmptyUriFirst => "aka_empty_uri_first",
            Self::AkaEmptyUriFirstNoAlgorithm => "aka_empty_uri_first_no_algorithm",
            Self::AkaZeroResponse => "aka_zero_response",
            Self::AkaZeroResponseUriFirst => "aka_zero_response_uri_first",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LiveRegisterRequestUri {
    HomeRegistrar,
    PcscfSocket,
}

impl LiveRegisterRequestUri {
    fn label(self) -> &'static str {
        match self {
            Self::HomeRegistrar => "home_registrar",
            Self::PcscfSocket => "pcscf_socket",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LiveRegisterIdentityFormat {
    ImsiHomeDomain,
    PrefixedImsiHomeDomain,
    ImsiPhoneUri,
    MsisdnPhoneUri,
}

impl LiveRegisterIdentityFormat {
    fn label(self) -> &'static str {
        match self {
            Self::ImsiHomeDomain => "imsi_home_domain",
            Self::PrefixedImsiHomeDomain => "prefixed_imsi_home_domain",
            Self::ImsiPhoneUri => "imsi_phone_uri",
            Self::MsisdnPhoneUri => "msisdn_phone_uri",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LiveRegisterHeaderProfile {
    contact_features: LiveContactFeatureSet,
    include_accept_contact: bool,
    include_p_preferred_identity: bool,
    visited_network: LiveVisitedNetworkFormat,
    pani: LivePaniFormat,
    include_cellular_network_info: bool,
    user_agent: LiveUserAgentFormat,
}

impl LiveRegisterHeaderProfile {
    const DEFAULT: Self = Self {
        contact_features: LiveContactFeatureSet::SmsOnly,
        include_accept_contact: false,
        include_p_preferred_identity: true,
        visited_network: LiveVisitedNetworkFormat::QuotedHome,
        pani: LivePaniFormat::ProfileDefault,
        include_cellular_network_info: true,
        user_agent: LiveUserAgentFormat::ProfileDefault,
    };

    const IMS_FEATURES: Self = Self {
        contact_features: LiveContactFeatureSet::MmtelSmsSipInstance,
        include_accept_contact: true,
        include_p_preferred_identity: true,
        visited_network: LiveVisitedNetworkFormat::QuotedHome,
        pani: LivePaniFormat::ProfileDefault,
        include_cellular_network_info: true,
        user_agent: LiveUserAgentFormat::ProfileDefault,
    };
}

#[derive(Debug, Clone, Copy)]
enum LiveContactFeatureSet {
    SmsOnly,
    MmtelSmsSipInstance,
}

impl LiveContactFeatureSet {
    fn label(self) -> &'static str {
        match self {
            Self::SmsOnly => "sms_only",
            Self::MmtelSmsSipInstance => "mmtel_sms_sip_instance",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LiveVisitedNetworkFormat {
    QuotedHome,
    UnquotedHome,
    Omit,
}

impl LiveVisitedNetworkFormat {
    fn label(self) -> &'static str {
        match self {
            Self::QuotedHome => "quoted_home",
            Self::UnquotedHome => "unquoted_home",
            Self::Omit => "omit",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LivePaniFormat {
    ProfileDefault,
    PlainWifi,
    Omit,
}

impl LivePaniFormat {
    fn label(self) -> &'static str {
        match self {
            Self::ProfileDefault => "profile_default",
            Self::PlainWifi => "plain_wifi",
            Self::Omit => "omit",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LiveUserAgentFormat {
    ProfileDefault,
    DeviceModelFocused,
}

impl LiveUserAgentFormat {
    fn label(self) -> &'static str {
        match self {
            Self::ProfileDefault => "profile_default",
            Self::DeviceModelFocused => "device_model_focused",
        }
    }
}

const LIVE_REGISTER_HEADER_VARIANTS: &[LiveRegisterHeaderVariant] = &[
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_uri_first_full_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_uri_first_minimal_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        security_client_format: LiveSecurityClientFormat::MinimalSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_uri_first_no_algorithm",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirstNoAlgorithm,
        security_client_format: LiveSecurityClientFormat::MinimalSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_uri_first_pcscf_uri",
        force_sec_agree_headers: false,
        include_route_header: false,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        security_client_format: LiveSecurityClientFormat::MinimalSpaced,
        request_uri: LiveRegisterRequestUri::PcscfSocket,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "profile_default_spaced_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::DEFAULT,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_spaced_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_placeholder",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_zero_placeholder",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaZeroResponse,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_no_security_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: false,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_plain_pani",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            pani: LivePaniFormat::PlainWifi,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_no_cellular",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            include_cellular_network_info: false,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_no_visited",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            visited_network: LiveVisitedNetworkFormat::Omit,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_empty_route_omitted",
        force_sec_agree_headers: false,
        include_route_header: false,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "msisdn_phone_uri_ims_features",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::MsisdnPhoneUri,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_plain_pani",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::MinimalSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            pani: LivePaniFormat::PlainWifi,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_no_cellular_info",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullCompact,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            include_cellular_network_info: false,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_no_preferred_identity",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            include_p_preferred_identity: false,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_unquoted_visited_network",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            visited_network: LiveVisitedNetworkFormat::UnquotedHome,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_no_visited_network",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            visited_network: LiveVisitedNetworkFormat::Omit,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_device_model_ua",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile {
            user_agent: LiveUserAgentFormat::DeviceModelFocused,
            ..LiveRegisterHeaderProfile::IMS_FEATURES
        },
    },
    LiveRegisterHeaderVariant {
        label: "ims_features_security_client_omitted",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: false,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "phone_uri_identity_ims_features",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiPhoneUri,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "prefixed_identity_ims_features",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::PrefixedImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "route_omitted_spaced_sec_client",
        force_sec_agree_headers: false,
        include_route_header: false,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "sec_agree_required_spaced_sec_client",
        force_sec_agree_headers: true,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
];

const GB_EE_REGISTER_HEADER_VARIANTS: &[LiveRegisterHeaderVariant] = &[
    LiveRegisterHeaderVariant {
        label: "gb_ee_aka_uri_first_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_no_initial_auth_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_aka_empty_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmpty,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_aka_zero_sec_client",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaZeroResponseUriFirst,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_aka_uri_first_required_sec_agree",
        force_sec_agree_headers: true,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_sec_agree_required",
        force_sec_agree_headers: true,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_prefixed_private_identity",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::PrefixedImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_phone_uri_identity",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiPhoneUri,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
    LiveRegisterHeaderVariant {
        label: "gb_ee_msisdn_public_identity",
        force_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::MsisdnPhoneUri,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
];

pub type LiveAdapterFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LiveStageObservation, LiveStageError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveStageObservation {
    pub stage: &'static str,
    pub ready: bool,
    pub detail: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveStageError {
    pub reason: String,
}

pub trait LiveStageAdapter: Send + Sync {
    fn run_stage<'a>(
        &'a self,
        stage: ExecutorStage,
        _profile: &'static CarrierProfile,
    ) -> LiveAdapterFuture<'a>;
}

pub trait LiveEpdgAdapter: Send + Sync {
    fn resolve_epdg<'a>(
        &'a self,
        _profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedEpdgEndpoint, LiveStageError>> + Send + 'a>>;
}

pub trait LiveDatagramAdapter: Send + Sync {
    fn check_udp_path<'a>(
        &'a self,
        stage: ExecutorStage,
        profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<(), LiveStageError>> + Send + 'a>>;
}

/// Resolves the ePDG using one line's overrides.
///
/// Holds the `line_id` so the lookup picks up that line's ePDG host/port, DNS
/// server and proxy — several SIMs can resolve different operators concurrently
/// without reading each other's settings.
#[derive(Debug, Clone, Default)]
pub struct SystemLiveEpdgAdapter {
    line_id: String,
}

impl SystemLiveEpdgAdapter {
    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self {
            line_id: line_id.into(),
        }
    }
}

impl LiveEpdgAdapter for SystemLiveEpdgAdapter {
    fn resolve_epdg<'a>(
        &'a self,
        profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedEpdgEndpoint, LiveStageError>> + Send + 'a>>
    {
        Box::pin(async move {
            match tokio::time::timeout(LIVE_DNS_TIMEOUT, resolve_live_epdg(&self.line_id, profile))
                .await
            {
                Ok(Ok(endpoint)) => Ok(endpoint),
                Ok(Err(_err)) => Err(live_stage_error("epdg_dns_resolution_failed")),
                Err(_) => Err(live_stage_error("epdg_dns_resolution_timeout")),
            }
        })
    }
}

/// Runs the live UDP-path stages for one line, carrying that line's ePDG/DNS/proxy
/// overrides so concurrent lines stay independent.
#[derive(Debug, Clone, Default)]
pub struct SystemLiveDatagramAdapter {
    line_id: String,
}

impl SystemLiveDatagramAdapter {
    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self {
            line_id: line_id.into(),
        }
    }
}

impl LiveDatagramAdapter for SystemLiveDatagramAdapter {
    fn check_udp_path<'a>(
        &'a self,
        stage: ExecutorStage,
        profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<(), LiveStageError>> + Send + 'a>> {
        Box::pin(async move {
            match stage {
                ExecutorStage::Ike => {
                    run_live_ike_until(&self.line_id, profile, LiveIkeTarget::EapSuccess)
                        .await
                        .map(|_| ())
                }
                ExecutorStage::ChildSa | ExecutorStage::Esp => {
                    run_live_esp_until(&self.line_id, profile).await
                }
                ExecutorStage::ImsRegister => {
                    run_live_ims_register_until(&self.line_id, profile).await
                }
                ExecutorStage::Sms => run_live_sms_until(&self.line_id, profile).await,
                ExecutorStage::Voice => run_live_voice_until(&self.line_id, profile).await,
                _ => Err(live_stage_error("packet_transport_stage_not_implemented")),
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatusProbeDatagramAdapter;

impl LiveDatagramAdapter for StatusProbeDatagramAdapter {
    fn check_udp_path<'a>(
        &'a self,
        stage: ExecutorStage,
        _profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<(), LiveStageError>> + Send + 'a>> {
        Box::pin(async move {
            match stage {
                ExecutorStage::Ike => Err(live_stage_error("status_probe_ike_deferred_to_connect")),
                _ => Err(live_stage_error("status_probe_stage_not_supported")),
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledDatagramAdapter;

impl LiveDatagramAdapter for DisabledDatagramAdapter {
    fn check_udp_path<'a>(
        &'a self,
        _stage: ExecutorStage,
        _profile: &'static CarrierProfile,
    ) -> Pin<Box<dyn Future<Output = Result<(), LiveStageError>> + Send + 'a>> {
        Box::pin(async { Err(live_stage_error("packet_transport_stage_not_implemented")) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveProbeDepth {
    StatusSaInit,
    FullHandshake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveIkeTarget {
    SaInitReady,
    EapSuccess,
    ChildSaReady,
}

struct LiveIkeSession {
    child_sa: Option<LiveChildSaMaterial>,
    transport: Option<UdpSocketDatagramTransport>,
    remote: Option<SocketAddr>,
}

struct LiveChildSaMaterial {
    inbound_sa_identifier: u32,
    outbound_sa_identifier: u32,
    selected_profile_proposal: &'static str,
    configuration: Option<IkeConfigurationMaterial>,
    secrets: ChildSaSecretPair,
}

#[derive(Debug, Clone)]
struct LiveIkeProposalGroup {
    dh_group: DhGroup,
    proposals: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct LiveIkeTransportPath {
    destination_port: u16,
    preferred_local_port: u16,
    initial_nat_t: bool,
    timeout_reason: &'static str,
}

const LIVE_IKE_TRANSPORT_PATHS: &[LiveIkeTransportPath] = &[
    LiveIkeTransportPath {
        destination_port: IKE_PORT,
        preferred_local_port: IKE_PORT,
        initial_nat_t: false,
        timeout_reason: "ike_sa_init_udp500_timeout",
    },
    LiveIkeTransportPath {
        destination_port: IKE_NAT_T_PORT,
        preferred_local_port: IKE_NAT_T_PORT,
        initial_nat_t: true,
        timeout_reason: "ike_sa_init_nat_t_4500_timeout",
    },
    LiveIkeTransportPath {
        destination_port: IKE_PORT,
        preferred_local_port: 0,
        initial_nat_t: false,
        timeout_reason: "ike_sa_init_udp500_ephemeral_source_timeout",
    },
    LiveIkeTransportPath {
        destination_port: IKE_NAT_T_PORT,
        preferred_local_port: 0,
        initial_nat_t: true,
        timeout_reason: "ike_sa_init_nat_t_4500_ephemeral_source_timeout",
    },
];

fn live_ike_proposal_groups(
    profile: &'static CarrierProfile,
) -> Result<Vec<LiveIkeProposalGroup>, LiveStageError> {
    let mut groups: Vec<LiveIkeProposalGroup> = Vec::new();
    for proposal in profile.ikev2.ike_proposals {
        let dh_transform = ike_proposal_dh_group_from_profile_string(proposal)
            .map_err(|_| live_stage_error("ike_profile_proposal_parse_failed"))?;
        let dh_group = DhGroup::from_transform_id(dh_transform)
            .ok_or_else(|| live_stage_error("ike_dh_group_unsupported"))?;

        if let Some(existing) = groups.iter_mut().find(|g| g.dh_group == dh_group) {
            existing.proposals.push(*proposal);
        } else {
            groups.push(LiveIkeProposalGroup {
                dh_group,
                proposals: vec![*proposal],
            });
        }
    }
    if groups.is_empty() {
        return Err(live_stage_error("ike_profile_missing_proposals"));
    }
    Ok(groups)
}

async fn run_live_ike_until(
    line_id: &str,
    profile: &'static CarrierProfile,
    target: LiveIkeTarget,
) -> Result<LiveIkeSession, LiveStageError> {
    run_live_ike_until_depth(line_id, profile, target, LiveProbeDepth::FullHandshake).await
}

async fn run_live_ike_until_depth(
    line_id: &str,
    profile: &'static CarrierProfile,
    target: LiveIkeTarget,
    depth: LiveProbeDepth,
) -> Result<LiveIkeSession, LiveStageError> {
    let (epdg_host, epdg_port, _) = live_epdg_settings(line_id, profile);
    info!("Resolving ePDG host: {} port: {}", epdg_host, epdg_port);
    let endpoint = tokio::time::timeout(LIVE_DNS_TIMEOUT, resolve_live_epdg(line_id, profile))
        .await
        .map_err(|_| live_stage_error("epdg_dns_resolution_timeout"))?
        .map_err(map_transport_error)?;
    let addresses = endpoint.addresses;
    info!("Resolved ePDG endpoint addresses: {:?}", addresses);
    if addresses.is_empty() {
        error!("No IP addresses found for ePDG");
        return Err(live_stage_error("epdg_no_address"));
    }

    let mut last_error = None;
    let endpoint_limit = match depth {
        LiveProbeDepth::StatusSaInit => 1,
        LiveProbeDepth::FullHandshake => LIVE_IKE_MAX_ENDPOINTS_PER_PASS,
    };
    let proposal_group_limit = match depth {
        LiveProbeDepth::StatusSaInit => 1,
        LiveProbeDepth::FullHandshake => LIVE_IKE_MAX_PROPOSAL_GROUPS_PER_PASS,
    };
    let transport_paths = match depth {
        LiveProbeDepth::StatusSaInit => &LIVE_IKE_TRANSPORT_PATHS[..1],
        LiveProbeDepth::FullHandshake => {
            &LIVE_IKE_TRANSPORT_PATHS[..LIVE_IKE_MAX_TRANSPORT_PATHS_PER_PASS]
        }
    };
    let proposal_groups = live_ike_proposal_groups(profile)?;
    let addresses = addresses
        .into_iter()
        .take(endpoint_limit)
        .collect::<Vec<_>>();
    let proposal_groups = proposal_groups
        .iter()
        .take(proposal_group_limit)
        .collect::<Vec<_>>();
    for proposal_group in proposal_groups {
        for mut destination in addresses.iter().copied() {
            for path in transport_paths {
                destination.set_port(path.destination_port);
                info!("Attempting connection path to destination={:?}, local_port_preferred={:?}, initial_nat_t={:?}", destination, path.preferred_local_port, path.initial_nat_t);
                match run_live_ike_with_destination(
                    line_id,
                    profile,
                    target,
                    destination,
                    *path,
                    proposal_group,
                )
                .await
                {
                    Ok(session) => {
                        info!(
                            selected_ike_proposals = ?proposal_group.proposals,
                            "Successfully established IKE session with destination={:?}",
                            destination,
                        );
                        return Ok(session);
                    }
                    Err(error) => {
                        warn!("Failed connection path to destination={:?}, local_port_preferred={:?}, error={:?}", destination, path.preferred_local_port, error);
                        last_error = Some(error);
                    }
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| live_stage_error("epdg_no_address")))
}

async fn run_live_ike_with_destination(
    line_id: &str,
    profile: &'static CarrierProfile,
    target: LiveIkeTarget,
    destination: SocketAddr,
    path: LiveIkeTransportPath,
    proposal_group: &LiveIkeProposalGroup,
) -> Result<LiveIkeSession, LiveStageError> {
    let local_addr = local_bind_addr_for_destination(destination, path.preferred_local_port)
        .await
        .unwrap_or_else(|_| unspecified_local_addr_for(destination));
    info!(
        "run_live_ike_with_destination: binding local_addr={:?} for destination={:?}",
        local_addr, destination
    );
    let transport = UdpSocketDatagramTransport::bind(local_addr)
        .await
        .map_err(map_transport_error)?
        .with_recv_timeout(LIVE_IKE_SA_INIT_TIMEOUT)
        .with_max_datagram_bytes(8192);

    let initiator_spi = generate_initiator_spi()?;
    let initiator_nonce = generate_nonce()?;
    debug!(
        nonce_len = initiator_nonce.len(),
        "Generated IKE initiator nonce metadata"
    );
    let dh = Modp2048Ephemeral::generate_for_group(proposal_group.dh_group)
        .map_err(|_| live_stage_error("ike_dh_material_unavailable"))?;
    let mut machine = IkeStateMachine::new_with_dh_group(
        profile,
        initiator_spi,
        initiator_nonce,
        dh.public_value().to_vec(),
        proposal_group.dh_group.transform_id(),
    );
    let local_addr = transport.local_addr().map_err(map_transport_error)?;
    let request = machine
        .build_sa_init_request_for_addresses_with_proposals(
            local_addr,
            destination,
            &proposal_group.proposals,
        )
        .map_err(|_| live_stage_error("ike_sa_init_request_build_failed"))?
        .encode()
        .map_err(|_| live_stage_error("ike_sa_init_request_encode_failed"))?;

    info!(
        "Sending IKE_SA_INIT request to destination={:?}, len={}, initial_nat_t={}",
        destination,
        request.len(),
        path.initial_nat_t
    );
    transport
        .send_ike_message_metadata(path.initial_nat_t, destination, &request)
        .await
        .map_err(map_transport_error)?;
    let response = recv_ike_response_with_retransmit(
        &transport,
        destination,
        &request,
        path.initial_nat_t,
        path.timeout_reason,
        LIVE_IKE_SA_INIT_ATTEMPTS,
    )
    .await?;
    info!("Received IKE_SA_INIT response, parsing...");
    if let Err(err) = machine.accept_sa_init_response(&response) {
        warn!("IKE_SA_INIT response rejected: {:?}", err);
        return Err(live_stage_error("ike_sa_init_response_rejected"));
    }
    info!("IKE_SA_INIT response parsed successfully");
    if target == LiveIkeTarget::SaInitReady {
        return Ok(LiveIkeSession {
            child_sa: None,
            transport: Some(transport.clone()),
            remote: Some(destination),
        });
    }
    let mut ike_destination = destination;
    let use_nat_t = path.initial_nat_t || machine.nat_t_supported();
    if use_nat_t {
        ike_destination.set_port(IKE_NAT_T_PORT);
    }
    info!(
        "IKE_AUTH destination port set to: {} (use_nat_t={})",
        ike_destination.port(),
        use_nat_t
    );
    let auth_transport = transport.clone().with_recv_timeout(LIVE_IKE_AUTH_TIMEOUT);
    let shared_secret = dh
        .shared_secret(
            machine
                .responder_public_dh()
                .ok_or_else(|| live_stage_error("ike_sa_init_missing_peer_dh"))?,
        )
        .map_err(|_| live_stage_error("ike_dh_shared_secret_failed"))?;
    debug!("Shared secret computed successfully");
    machine
        .derive_session_keys(&shared_secret)
        .map_err(|_| live_stage_error("ike_session_key_derivation_failed"))?;
    let identity = live_ike_identity(line_id, profile).await?;
    info!(
        identity_len = identity.len(),
        "Resolved NAI identity for IKE_AUTH"
    );
    let auth_packet = machine
        .build_auth_eap_start_packet_for_identity(&identity)
        .map_err(|_| live_stage_error("ike_auth_request_build_failed"))?;
    info!(
        "Sending IKE_AUTH EAP start request to {:?}",
        ike_destination
    );
    auth_transport
        .send_ike_message_metadata(use_nat_t, ike_destination, &auth_packet)
        .await
        .map_err(map_transport_error)?;
    let auth_response = recv_ike_response_with_retransmit(
        &auth_transport,
        ike_destination,
        &auth_packet,
        use_nat_t,
        "ike_auth_challenge_timeout",
        LIVE_IKE_AUTH_ATTEMPTS,
    )
    .await?;
    info!("Received IKE_AUTH challenge response, validating header...");
    validate_ike_auth_response(&auth_response, initiator_spi, 1)?;
    machine
        .accept_encrypted_eap_aka_challenge_reason(&auth_response)
        .map_err(|reason| {
            error!("EAP-AKA challenge accept failed: {}", reason);
            LiveStageError { reason }
        })?;
    info!("Decrypting EAP-AKA challenge...");
    let eap_challenge = machine
        .decrypted_eap_aka_challenge_packet(&auth_response)
        .map_err(|_| live_stage_error("ike_auth_eap_challenge_decode_failed"))?;
    let challenge = parse_challenge(&eap_challenge)
        .map_err(|_| live_stage_error("eap_aka_challenge_parse_failed"))?;
    info!("Spawning USIM Authentication via QMI proxy...");
    // Authenticate against THIS line's reader. Using the global device would make
    // a second line run EAP-AKA against the first line's card, which fails
    // authentication (or worse, succeeds with the wrong subscriber identity).
    let proxy_socket = live_runtime_config().qmi_proxy_socket;
    let sim_device = sim_device_for_line(line_id);
    let aka_result = tokio::task::spawn_blocking({
        let rand = challenge.rand.clone();
        let autn = challenge.autn.clone();
        move || {
            execute_usim_authenticate_via_proxy_reason_with_retry(
                proxy_socket.as_str(),
                sim_device.qmi_device.as_str(),
                sim_device.uim_slot,
                USIM_AID_PREFIX,
                &rand,
                &autn,
                LIVE_SIM_AUTH_ATTEMPTS,
                LIVE_SIM_AUTH_TIMEOUT,
                LIVE_SIM_AUTH_RETRY_DELAY,
            )
        }
    })
    .await
    .map_err(|_| live_stage_error("sim_auth_runtime_failed"))?
    .map_err(|reason| {
        error!("USIM Authentication via QMI proxy failed: {}", reason);
        live_stage_error(reason)
    })?;
    info!(
        "USIM Authentication returned successfully, auts present: {}",
        aka_result.auts.is_some()
    );
    let mut eap_response = if let Some(auts) = aka_result.auts.as_deref() {
        build_sync_failure_response(&challenge, auts)
            .map_err(|_| live_stage_error("eap_aka_response_build_failed"))?
    } else {
        build_challenge_response(&challenge, &identity, &aka_result)
            .map_err(|_| live_stage_error("eap_aka_response_build_failed"))?
    };
    let eap_response_packet = machine
        .build_encrypted_eap_response_packet(eap_response.expose_for_ike_encryption())
        .map_err(|_| live_stage_error("ike_auth_eap_response_encrypt_failed"))?;
    info!(
        "Sending EAP-AKA challenge response packet to {:?}",
        ike_destination
    );
    auth_transport
        .send_ike_message_metadata(use_nat_t, ike_destination, &eap_response_packet)
        .await
        .map_err(map_transport_error)?;
    let mut last_auth_request = eap_response_packet;

    let mut success_includes_child_sa = false;
    for loop_idx in 0..5 {
        let expected_message_id = machine.next_message_id().saturating_sub(1);
        debug!(
            "EAP progress loop {}, expected_message_id={}",
            loop_idx, expected_message_id
        );
        let auth_progress_response = recv_ike_response_with_retransmit(
            &auth_transport,
            ike_destination,
            &last_auth_request,
            use_nat_t,
            "ike_auth_progress_timeout",
            LIVE_IKE_AUTH_ATTEMPTS,
        )
        .await?;
        validate_ike_auth_response(&auth_progress_response, initiator_spi, expected_message_id)?;
        match machine
            .accept_encrypted_auth_progress_or_reason(&auth_progress_response)
            .map_err(|reason| {
                error!("EAP progress accept failed: {}", reason);
                LiveStageError { reason }
            })? {
            IkeAuthProgress::EapAkaIdentity { packet } => {
                info!("Received EapAkaIdentity request from ePDG");
                eap_response = eap_response
                    .identity_response(&packet, &identity)
                    .map_err(|_| live_stage_error("eap_aka_identity_response_build_failed"))?;
                let identity_response_packet = machine
                    .build_encrypted_eap_response_packet(eap_response.expose_for_ike_encryption())
                    .map_err(|_| live_stage_error("ike_auth_eap_identity_encrypt_failed"))?;
                info!("Sending EapAkaIdentity response to {:?}", ike_destination);
                auth_transport
                    .send_ike_message_metadata(
                        use_nat_t,
                        ike_destination,
                        &identity_response_packet,
                    )
                    .await
                    .map_err(map_transport_error)?;
                last_auth_request = identity_response_packet;
            }
            IkeAuthProgress::EapSuccess { child_sa_included } => {
                info!(
                    "Received EapSuccess from ePDG, child_sa_included={}",
                    child_sa_included
                );
                success_includes_child_sa = child_sa_included;
                break;
            }
            IkeAuthProgress::EapAkaNotification { packet } => {
                info!("Received EapAkaNotification request from ePDG");
                eap_response = eap_response
                    .notification_response(&packet)
                    .map_err(|_| live_stage_error("eap_aka_notification_response_build_failed"))?;
                let notification_response_packet = machine
                    .build_encrypted_eap_response_packet(eap_response.expose_for_ike_encryption())
                    .map_err(|_| live_stage_error("ike_auth_eap_notification_encrypt_failed"))?;
                info!(
                    "Sending EapAkaNotification response to {:?}",
                    ike_destination
                );
                auth_transport
                    .send_ike_message_metadata(
                        use_nat_t,
                        ike_destination,
                        &notification_response_packet,
                    )
                    .await
                    .map_err(map_transport_error)?;
                last_auth_request = notification_response_packet;
            }
        }
    }
    if machine.snapshot().phase != "auth_success_accepted"
        && machine.snapshot().phase != "child_sa_ready"
    {
        error!(
            "EAP-AKA success phase not reached. Current phase: {}",
            machine.snapshot().phase
        );
        return Err(live_stage_error("eap_aka_success_not_reached"));
    }

    if target == LiveIkeTarget::ChildSaReady && !success_includes_child_sa {
        info!("Child SA not included in EapSuccess. Building final IKE_AUTH request...");
        let msk = eap_response
            .msk_for_ike_auth()
            .ok_or_else(|| live_stage_error("eap_aka_msk_unavailable"))?;
        let expected_message_id = machine.next_message_id();
        let final_auth_packet = machine
            .build_encrypted_final_auth_packet(msk)
            .map_err(|_| live_stage_error("ike_auth_final_request_build_failed"))?;
        info!("Sending final IKE_AUTH request to {:?}", ike_destination);
        auth_transport
            .send_ike_message_metadata(use_nat_t, ike_destination, &final_auth_packet)
            .await
            .map_err(map_transport_error)?;
        let child_sa_response = recv_ike_response_with_retransmit(
            &auth_transport,
            ike_destination,
            &final_auth_packet,
            use_nat_t,
            "ike_child_sa_timeout",
            LIVE_IKE_AUTH_ATTEMPTS,
        )
        .await?;
        info!("Received final IKE_AUTH response, validating...");
        validate_ike_auth_response(&child_sa_response, initiator_spi, expected_message_id)?;
        machine
            .accept_encrypted_child_sa_response_or_reason(&child_sa_response)
            .map_err(|reason| LiveStageError { reason })?;
    }

    Ok(LiveIkeSession {
        child_sa: machine
            .child_sa_material()
            .map(|material| LiveChildSaMaterial {
                inbound_sa_identifier: material.inbound_sa_identifier,
                outbound_sa_identifier: material.outbound_sa_identifier,
                selected_profile_proposal: material.selected_profile_proposal,
                configuration: material.configuration.clone(),
                secrets: material.secrets.clone(),
            }),
        transport: Some(auth_transport.clone()),
        remote: Some(ike_destination),
    })
}

async fn run_live_esp_until(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<(), LiveStageError> {
    if cached_tun_gateway_matches(line_id, profile).await {
        return Ok(());
    }

    info!("Live ESP stage check: building full ePDG IKE/EAP-AKA/CHILD_SA path...");
    let session = run_live_ike_until(line_id, profile, LiveIkeTarget::ChildSaReady).await?;
    let child_sa = session
        .child_sa
        .as_ref()
        .ok_or_else(|| live_stage_error("live_child_sa_material_missing"))?;
    let mut dataplane = ChildSaStateMachine::new(profile);
    dataplane
        .negotiate_child_sa_with_profile_proposal(
            child_sa.inbound_sa_identifier,
            child_sa.outbound_sa_identifier,
            child_sa.selected_profile_proposal,
        )
        .map_err(map_dataplane_state_error)?;
    dataplane
        .mark_esp_secrets_ready()
        .map_err(map_dataplane_state_error)?;
    dataplane
        .mark_inner_stack_ready()
        .map_err(map_dataplane_state_error)?;
    let snapshot = dataplane.snapshot();
    if snapshot.phase != "inner_stack_ready" {
        return Err(live_stage_error("live_esp_inner_stack_not_ready"));
    }
    ensure_live_tun_gateway(line_id, profile, &session, child_sa).await
}

fn tun_gateway_cache() -> &'static Mutex<HashMap<String, Arc<TunGatewayRuntime>>> {
    LIVE_TUN_GATEWAY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether this line already has a gateway for this carrier profile.
///
/// Both keys matter: the entry is looked up by line (so lines never share a
/// tunnel) and then checked against the profile (so a line that switched carriers
/// rebuilds instead of reusing a stale tunnel).
async fn cached_tun_gateway_matches(line_id: &str, profile: &'static CarrierProfile) -> bool {
    tun_gateway_cache()
        .lock()
        .await
        .get(line_id)
        .map(|runtime| runtime.is_for_profile(profile.meta.profile_id))
        .unwrap_or(false)
}

async fn ensure_live_tun_gateway(
    line_id: &str,
    profile: &'static CarrierProfile,
    session: &LiveIkeSession,
    child_sa: &LiveChildSaMaterial,
) -> Result<(), LiveStageError> {
    let configuration = child_sa
        .configuration
        .as_ref()
        .ok_or_else(|| live_stage_error("live_child_sa_configuration_missing"))?;
    let inner_addr = select_inner_address(profile, configuration)
        .ok_or_else(|| live_stage_error("live_inner_address_missing"))?;
    let pcscf_addr = select_pcscf_address(profile, configuration, inner_addr)
        .ok_or_else(|| live_stage_error("live_pcscf_address_missing"))?;
    let transport = session
        .transport
        .clone()
        .ok_or_else(|| live_stage_error("live_transport_missing"))?;
    let remote = session
        .remote
        .ok_or_else(|| live_stage_error("live_remote_endpoint_missing"))?;

    let gateway = tun_gateway::start_gateway(TunGatewayConfig {
        profile_id: profile.meta.profile_id,
        // Per-line device name: two connected lines must not contend for one
        // interface, and the name stays stable so a reconnect reclaims its own.
        tun_name: tun_name_for_line(&live_runtime_config().tun_name, line_id),
        inner_addr,
        inner_prefix_len: configuration.assigned_ipv6_prefix_length,
        pcscf_addr,
        pcscf_addrs: pcscf_candidates(profile, configuration, inner_addr),
        inbound_sa_identifier: child_sa.inbound_sa_identifier,
        outbound_sa_identifier: child_sa.outbound_sa_identifier,
        secrets: child_sa.secrets.clone(),
        transport,
        remote,
    })
    .await
    .map_err(|error| live_stage_error(error.reason()))?;

    tun_gateway_cache()
        .lock()
        .await
        .insert(line_id.to_string(), gateway);
    Ok(())
}

fn select_inner_address(
    profile: &'static CarrierProfile,
    configuration: &IkeConfigurationMaterial,
) -> Option<IpAddr> {
    if profile.epdg.ip_stack.contains("ipv6") {
        if let Some(addr) = configuration
            .assigned_inner_addresses
            .iter()
            .copied()
            .find(IpAddr::is_ipv6)
        {
            return Some(addr);
        }
    }
    configuration.assigned_inner_addresses.first().copied()
}

fn select_pcscf_address(
    profile: &'static CarrierProfile,
    configuration: &IkeConfigurationMaterial,
    inner_addr: IpAddr,
) -> Option<IpAddr> {
    if let Some(addr) = configuration
        .pcscf_addresses
        .iter()
        .copied()
        .find(|addr| addr.is_ipv4() == inner_addr.is_ipv4())
    {
        return Some(addr);
    }

    profile
        .ims
        .pcscf
        .and_then(|pcscf| pcscf.parse::<IpAddr>().ok())
        .filter(|addr| addr.is_ipv4() == inner_addr.is_ipv4())
}

fn pcscf_candidates(
    profile: &'static CarrierProfile,
    configuration: &IkeConfigurationMaterial,
    inner_addr: IpAddr,
) -> Vec<IpAddr> {
    let mut addrs = configuration
        .pcscf_addresses
        .iter()
        .copied()
        .filter(|addr| addr.is_ipv4() == inner_addr.is_ipv4())
        .collect::<Vec<_>>();
    if let Some(static_addr) = profile
        .ims
        .pcscf
        .and_then(|pcscf| pcscf.parse::<IpAddr>().ok())
        .filter(|addr| addr.is_ipv4() == inner_addr.is_ipv4())
    {
        addrs.push(static_addr);
    }
    addrs.sort();
    addrs.dedup();
    addrs
}

async fn run_live_ims_register_until(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<(), LiveStageError> {
    info!("Live ImsRegister stage check: verifying outer ESP tunnel and IMS TCP path...");
    run_live_esp_until(line_id, profile).await?;
    let gateway = cached_tun_gateway(line_id, profile).await?;
    let response = run_register_exchange_over_tunnel(line_id, profile, &gateway).await?;
    let parsed = ims::parse_sip_response(&response, profile.ims.realm)
        .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
    info!(
        status_code = parsed.status_code,
        reason = parsed.reason.as_str(),
        service_route_present = parsed.service_route_present,
        associated_uri_count = parsed.associated_uri_count,
        warning_present = parsed.warning_present,
        unsupported = ?parsed.unsupported,
        require = ?parsed.require,
        proxy_require = ?parsed.proxy_require,
        "IMS REGISTER final response metadata received"
    );

    match parsed.status_code {
        200 => {
            record_live_ims_register_ready(line_id, profile, true, parsed.expires_seconds).await;
            Ok(())
        }
        401 | 407 => Err(live_stage_error("ims_register_auth_rejected")),
        _ => Err(live_stage_error("ims_register_unexpected_status")),
    }
}

async fn run_live_sms_until(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<(), LiveStageError> {
    info!("Live Sms stage check: verifying protected IMS registration and SMSIP readiness...");
    match profile.sms.receiver_transport {
        "tcp" | "udp" => {}
        _ => return Err(live_stage_error("sms_receiver_transport_unsupported")),
    }

    if !cached_live_ims_register_ready(line_id, profile).await {
        run_live_ims_register_until(line_id, profile).await?;
    }

    let mut sms_state = sms::SmsRuntimeStateMachine::new(profile);
    sms_state.mark_subscribe_reg_ready();
    sms_state
        .assert_state_consistency()
        .map_err(|_| live_stage_error("sms_state_inconsistent"))?;
    info!(
        receiver_transport = profile.sms.receiver_transport,
        sms_capability_advertised = true,
        "SMS over IMS signaling readiness validated"
    );
    Ok(())
}

/// Voice-over-IMS stage readiness check. Validates that IMS registration is in
/// place and that at least one voice leg (VoWiFi or a USB-Audio backed carrier
/// leg) is configured for the profile, mirroring `run_live_sms_until`.
///
/// The actual media path (RTP over the ESP inner stack) is exercised only when
/// a call is placed; this stage only confirms signaling prerequisites.
async fn run_live_voice_until(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<(), LiveStageError> {
    info!("Live Voice stage check: verifying IMS registration and voice leg readiness...");
    if !profile.voice.vowifi_enabled && !profile.voice.carrier_fallback_enabled {
        return Err(live_stage_error("voice_no_leg_enabled"));
    }

    if !cached_live_ims_register_ready(line_id, profile).await {
        run_live_ims_register_until(line_id, profile).await?;
    }

    let mut voice_state = voice::VoiceCallStateMachine::new(profile);
    voice_state.mark_registration_ready();
    voice_state
        .assert_state_consistency()
        .map_err(|_| live_stage_error("voice_state_inconsistent"))?;
    info!(
        vowifi_voice_enabled = profile.voice.vowifi_enabled,
        carrier_fallback_enabled = profile.voice.carrier_fallback_enabled,
        preferred_codec = profile
            .voice
            .preferred_codecs
            .first()
            .copied()
            .unwrap_or("none"),
        "Voice over IMS signaling readiness validated"
    );
    Ok(())
}

/// Place a live VoWiFi voice call to `callee`.
///
/// This resolves the SIM identity/profile, ensures the ESP-protected IMS
/// registration is current, allocates a local RTP media port on the inner
/// stack, builds an SDP offer with the profile's codecs, and sends an INVITE
/// over the same ESP TCP route used for SMS. Dialog/media progress is reported
/// asynchronously through the returned follow-up channel.
///
/// The RTP media loop itself is intentionally reserved: `place_live_voice_call`
/// establishes the signaling dialog and negotiates the codec, then hands the
/// media session to the (pluggable) audio backend via the reserved
/// [`voice::AudioSource`]/[`voice::AudioSink`] interfaces. Until a backend is
/// bound, media flows as silence.
pub async fn place_live_voice_call(callee: &str) -> Result<LiveCallResult, LiveStageError> {
    place_live_voice_call_for_line("", callee).await
}

/// Place a call using one line's network overrides.
///
/// `line_id` selects that line's ePDG/DNS/proxy settings; an empty value falls
/// back to carrier-profile defaults.
pub async fn place_live_voice_call_for_line(
    line_id: &str,
    callee: &str,
) -> Result<LiveCallResult, LiveStageError> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("voice_identity_unavailable"))?;
    // Resolve the carrier from THIS line's SIM, so a second line does not place
    // the call through the first line's operator profile.
    let identity = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("voice_identity_unavailable"))?;
    let profile_match = resolve_profile_for_line(line_id, identity.imsi.trim())
        .ok_or_else(|| live_stage_error("voice_profile_unmatched"))?;
    let profile = profile_match.profile;

    if !profile.voice.vowifi_enabled {
        return Err(live_stage_error("voice_vowifi_leg_disabled"));
    }

    match tokio::time::timeout(
        LIVE_VOICE_INVITE_TOTAL_TIMEOUT,
        place_live_voice_call_for_profile(&conn, line_id, profile, callee),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            warn!(
                profile_id = profile.meta.profile_id,
                timeout_ms = LIVE_VOICE_INVITE_TOTAL_TIMEOUT.as_millis() as u64,
                "VoWiFi voice INVITE timed out; IMS session cache will be cleared"
            );
            clear_live_ims_session(line_id, profile).await;
            Err(live_stage_error("voice_invite_timeout"))
        }
    }
}

async fn place_live_voice_call_for_profile(
    _conn: &zbus::Connection,
    line_id: &str,
    profile: &'static CarrierProfile,
    callee: &str,
) -> Result<LiveCallResult, LiveStageError> {
    if !cached_live_ims_register_ready(line_id, profile).await {
        info!(
            profile_id = profile.meta.profile_id,
            "VoWiFi voice call refreshing IMS registration before INVITE"
        );
        run_live_ims_register_until(line_id, profile).await?;
    }
    let gateway = cached_tun_gateway(line_id, profile).await?;
    let route = gateway
        .ims_client_tcp_route()
        .map_err(|error| live_stage_error(error.reason()))?;
    if route.profile_id != profile.meta.profile_id {
        return Err(live_stage_error("voice_ims_policy_profile_mismatch"));
    }

    let identity =
        live_ims_register_identity(line_id, profile, LiveRegisterIdentityFormat::ImsiHomeDomain)
            .await?;
    let security_verify = cached_live_ims_security_verify(line_id, profile).await;
    let callee_user = sip_phone_user(callee)?;
    let request_uri = format!("sip:{callee_user}@{};user=phone", profile.ims.domain);

    // Local media address is the inner-stack address; the media port is chosen
    // in the standard dynamic RTP range. The RTP socket itself is bound by the
    // media backend when one is attached.
    let media_addr = match route.local_addr {
        IpAddr::V4(_) => voice::SdpAddrType::Ip4,
        IpAddr::V6(_) => voice::SdpAddrType::Ip6,
    };
    let media_port = live_voice_media_port();
    let connection_addr = sip_host(route.local_addr);
    let offer = voice::build_mo_audio_offer(profile, &connection_addr, media_addr, media_port);
    let offered_codecs = offer.codecs.iter().map(|codec| codec.codec).collect();
    let sdp_offer = offer.to_sdp().into_bytes();

    let invite = voice::MoCallInvite {
        trace_id: format!("voice-mo-{}", hex_token(8)),
        call_id: format!("{}@simadmin", hex_token(16)),
        callee: callee.to_string(),
        sdp_bytes: sdp_offer.len(),
        sdp_offer,
        offered_codecs,
    };

    info!(
        profile_id = profile.meta.profile_id,
        sdp_bytes = invite.sdp_bytes,
        media_port,
        pcscf_family = ip_family_name(route.remote_addr),
        security_verify_present = security_verify.is_some(),
        "VoWiFi MO voice INVITE prepared"
    );

    send_live_invite(
        profile,
        &route,
        &identity,
        &invite,
        &request_uri,
        security_verify.as_deref(),
    )
    .await
}

/// The local RTP media port used for the SDP offer. A fixed port in the dynamic
/// range keeps the reserved media path deterministic; the real backend may
/// override this when it binds the socket.
fn live_voice_media_port() -> u16 {
    40000
}

async fn send_live_invite(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    invite: &voice::MoCallInvite,
    request_uri: &str,
    security_verify: Option<&str>,
) -> Result<LiveCallResult, LiveStageError> {
    let target = SocketAddr::new(route.remote_addr, route.remote_port);
    let mut stream = connect_tcp_from_inner(route.local_addr, target, route.local_port).await?;
    let local_addr = stream
        .local_addr()
        .map_err(|_| live_stage_error("voice_tcp_local_addr_unavailable"))?;
    let mut pending = Vec::new();

    let request = build_live_invite_request(
        profile,
        route,
        identity,
        invite,
        request_uri,
        security_verify,
        local_addr,
    );
    if let Err(err) = write_sip_frame(&mut stream, &request).await {
        abort_tcp_stream(stream);
        return Err(err);
    }

    // Read responses until a final (>=200) response arrives, sending ACK on 2xx.
    let mut outcome = voice::MoCallSipOutcome {
        trace_id: invite.trace_id.clone(),
        call_id: invite.call_id.clone(),
        sip_status: 0,
        invite_state: voice::SipInviteState::InviteSent,
        call_state: voice::CallState::Dialing,
        negotiated_codec: None,
        failure_cause: None,
    };

    let (final_status, final_frame) = loop {
        let frame = match read_sip_frame_buffered(
            &mut stream,
            &mut pending,
            LIVE_IMS_REGISTER_READ_TIMEOUT,
            "voice_invite_response_timeout",
        )
        .await
        {
            Ok(frame) => frame,
            Err(err) => {
                abort_tcp_stream(stream);
                return Err(err);
            }
        };
        let status = match parse_sip_status(&frame) {
            Ok(status) => status,
            Err(_) => continue,
        };
        outcome.sip_status = status;
        if (100..200).contains(&status) {
            outcome.invite_state = if status == 183 {
                voice::SipInviteState::EarlyMedia
            } else {
                voice::SipInviteState::Ringing
            };
            outcome.call_state = voice::CallState::Ringing;
            info!(
                profile_id = profile.meta.profile_id,
                status_code = status,
                "VoWiFi voice INVITE provisional response received"
            );
            continue;
        }
        break (status, frame);
    };

    if !(200..300).contains(&final_status) {
        outcome.invite_state = voice::SipInviteState::Failed;
        outcome.call_state = voice::CallState::Failed;
        outcome.failure_cause = Some(format!("sip_{final_status}"));
        abort_tcp_stream(stream);
        return Ok(start_live_call_followup_task(profile, outcome, None));
    }

    // Parse the SDP answer to learn the negotiated codec (the answer body
    // follows the 200 OK headers). We pick the first codec both sides support.
    let negotiated_codec = voice::parse_audio_sdp(sip_body(&final_frame))
        .ok()
        .and_then(|answer| {
            let offer = voice::build_profile_codec_offer(profile);
            answer
                .codecs
                .iter()
                .find(|remote| offer.iter().any(|local| local.codec == remote.codec))
                .map(|codec| codec.codec)
        });
    outcome.invite_state = voice::SipInviteState::Confirmed;
    outcome.call_state = voice::CallState::Active;
    outcome.negotiated_codec = negotiated_codec;

    // Send ACK to confirm the dialog.
    let ack = build_live_ack_request(
        profile,
        route,
        identity,
        invite,
        request_uri,
        security_verify,
        local_addr,
    );
    if let Err(err) = write_sip_frame(&mut stream, &ack).await {
        warn!(
            profile_id = profile.meta.profile_id,
            reason = err.reason.as_str(),
            "VoWiFi voice ACK write failed after 200 OK"
        );
    }

    info!(
        profile_id = profile.meta.profile_id,
        status_code = final_status,
        "VoWiFi voice call confirmed (dialog established, media reserved)"
    );

    Ok(start_live_call_followup_task(
        profile,
        outcome,
        Some((stream, pending)),
    ))
}

/// Spawn the follow-up task for a voice call. It reports the confirmed/failed
/// outcome immediately, then (for confirmed calls) holds the dialog until the
/// media window ends. The RTP media loop is reserved: when an audio backend is
/// attached it runs here, wrapping [`voice::AudioSource`] frames into RTP via
/// [`voice::RtpPacket`] and delivering received RTP to the [`voice::AudioSink`].
fn start_live_call_followup_task(
    _profile: &'static CarrierProfile,
    outcome: voice::MoCallSipOutcome,
    dialog: Option<(TcpStream, Vec<u8>)>,
) -> LiveCallResult {
    let (tx, rx) = mpsc::unbounded_channel();
    let seed = outcome.clone();
    tokio::spawn(async move {
        // Report the initial outcome (confirmed or failed).
        let _ = tx.send(LiveCallFollowupFrame {
            outcome: seed.clone(),
        });

        let Some((mut stream, mut pending)) = dialog else {
            return;
        };
        if seed.call_state != voice::CallState::Active {
            abort_tcp_stream(stream);
            return;
        }

        // Reserved media window: drive the RTP loop here once an audio backend
        // is bound. Until then, watch the signaling channel for an in-dialog
        // BYE so the call terminates cleanly, and report the ended outcome.
        let deadline = tokio::time::Instant::now() + LIVE_SMS_FOLLOWUP_WINDOW;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let timeout = std::cmp::min(
                Duration::from_secs(6),
                deadline.saturating_duration_since(now),
            );
            match read_sip_frame_buffered(
                &mut stream,
                &mut pending,
                timeout,
                "voice_dialog_idle_timeout",
            )
            .await
            {
                Ok(frame) if sip_frame_is_request(&frame, "BYE") => {
                    if let Ok(response) = build_sip_ok_response_for_request(&frame) {
                        let _ = write_sip_frame(&mut stream, &response).await;
                    }
                    let mut ended = seed.clone();
                    ended.invite_state = voice::SipInviteState::Terminated;
                    ended.call_state = voice::CallState::Ended;
                    let _ = tx.send(LiveCallFollowupFrame { outcome: ended });
                    break;
                }
                Ok(_) => continue,
                Err(err) if err.reason == "voice_dialog_idle_timeout" => break,
                Err(_) => break,
            }
        }
        abort_tcp_stream(stream);
    });

    LiveCallResult {
        outcome,
        followup: rx,
    }
}

fn live_ims_register_cache_ttl(expires_seconds: Option<u32>) -> Duration {
    let Some(expires_seconds) = expires_seconds else {
        return LIVE_IMS_REGISTER_DEFAULT_TTL;
    };
    let mut ttl = Duration::from_secs(u64::from(expires_seconds));
    ttl = ttl.saturating_sub(LIVE_IMS_REGISTER_EXPIRY_SKEW);
    ttl.clamp(LIVE_IMS_REGISTER_DEFAULT_TTL, LIVE_IMS_REGISTER_MAX_TTL)
}

fn ims_register_ready_cache() -> &'static Mutex<HashMap<String, LiveImsRegisterReady>> {
    LIVE_IMS_REGISTER_READY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_security_verify_cache() -> &'static Mutex<HashMap<String, LiveImsSecurityVerify>> {
    LIVE_IMS_SECURITY_VERIFY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_tcp_channel_cache() -> &'static Mutex<HashMap<String, LiveImsTcpChannel>> {
    LIVE_IMS_TCP_CHANNEL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_register_variant_cache() -> &'static Mutex<HashMap<String, LiveImsRegisterSuccessVariant>> {
    LIVE_IMS_REGISTER_SUCCESS_VARIANT.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn record_live_ims_register_ready(
    line_id: &str,
    profile: &'static CarrierProfile,
    sms_capability_advertised: bool,
    expires_seconds: Option<u32>,
) {
    let ttl = live_ims_register_cache_ttl(expires_seconds);
    ims_register_ready_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsRegisterReady {
            profile_id: profile.meta.profile_id,
            expires_at: Instant::now() + ttl,
            sms_capability_advertised,
            receiver_transport: profile.sms.receiver_transport,
        },
    );
    info!(
        line_id,
        profile_id = profile.meta.profile_id,
        ttl_secs = ttl.as_secs(),
        expires_seconds,
        "IMS REGISTER ready cache updated"
    );
}

async fn cached_live_ims_register_ready(line_id: &str, profile: &'static CarrierProfile) -> bool {
    ims_register_ready_cache()
        .lock()
        .await
        .get(line_id)
        .filter(|ready| ready.profile_id == profile.meta.profile_id)
        .filter(|ready| ready.sms_capability_advertised)
        .filter(|ready| ready.receiver_transport == profile.sms.receiver_transport)
        .is_some_and(|ready| ready.expires_at > Instant::now())
}

async fn cached_live_ims_expires_at(line_id: &str, profile: &'static CarrierProfile) -> Instant {
    ims_register_ready_cache()
        .lock()
        .await
        .get(line_id)
        .filter(|ready| ready.profile_id == profile.meta.profile_id)
        .map(|ready| ready.expires_at)
        .unwrap_or_else(|| Instant::now() + LIVE_IMS_REGISTER_DEFAULT_TTL)
}

async fn record_live_ims_security_verify(
    line_id: &str,
    profile: &'static CarrierProfile,
    security_verify: Option<&str>,
    expires_seconds: Option<u32>,
) {
    let Some(value) = security_verify.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    ims_security_verify_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsSecurityVerify {
            profile_id: profile.meta.profile_id,
            expires_at: Instant::now() + live_ims_register_cache_ttl(expires_seconds),
            value: value.to_string(),
        },
    );
}

async fn cached_live_ims_security_verify(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Option<String> {
    ims_security_verify_cache()
        .lock()
        .await
        .get(line_id)
        .filter(|ready| ready.profile_id == profile.meta.profile_id)
        .filter(|ready| ready.expires_at > Instant::now())
        .map(|ready| ready.value.clone())
}

#[allow(clippy::too_many_arguments)]
async fn record_live_ims_tcp_channel(
    line_id: &str,
    profile: &'static CarrierProfile,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    stream: TcpStream,
    security_verify: Option<String>,
    expires_seconds: Option<u32>,
) {
    ims_tcp_channel_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsTcpChannel {
            profile_id: profile.meta.profile_id,
            expires_at: Instant::now() + live_ims_register_cache_ttl(expires_seconds),
            channel: super::channel::EpdgSipChannel::new(
                stream,
                Vec::new(),
                crate::connectivity::core::context::ImsRoute {
                    local_addr,
                    pcscf_addr: remote_addr,
                    transport: crate::connectivity::core::context::SipTransport::Tcp,
                },
                security_verify,
            ),
        },
    );
}

async fn clear_live_ims_tcp_channel(line_id: &str, profile: &'static CarrierProfile) {
    let mut cache = ims_tcp_channel_cache().lock().await;
    if cache
        .get(line_id)
        .is_some_and(|channel| channel.profile_id == profile.meta.profile_id)
    {
        cache.remove(line_id);
    }
}

/// Tear down every line's live runtime. Process-wide shutdown only — for a single
/// line use [`clear_live_runtime_for_line`], which leaves other lines connected.
pub async fn clear_all_live_runtime() {
    let channels: Vec<LiveImsTcpChannel> = ims_tcp_channel_cache()
        .lock()
        .await
        .drain()
        .map(|(_, channel)| channel)
        .collect();
    for channel in channels {
        let (stream, _) = channel.channel.into_parts();
        abort_tcp_stream(stream);
    }

    ims_register_ready_cache().lock().await.clear();
    ims_security_verify_cache().lock().await.clear();
    ims_register_variant_cache().lock().await.clear();

    let gateways: Vec<Arc<TunGatewayRuntime>> = tun_gateway_cache()
        .lock()
        .await
        .drain()
        .map(|(_, gateway)| gateway)
        .collect();
    for gateway in gateways {
        gateway.shutdown();
    }
}

/// Tear down one line's live runtime, leaving every other line untouched.
///
/// Needed because resetting a single line previously went through
/// [`clear_all_live_runtime`], which killed every other line's tunnel too.
pub async fn clear_live_runtime_for_line(line_id: &str) {
    if let Some(channel) = ims_tcp_channel_cache().lock().await.remove(line_id) {
        let (stream, _) = channel.channel.into_parts();
        abort_tcp_stream(stream);
    }
    ims_register_ready_cache().lock().await.remove(line_id);
    ims_security_verify_cache().lock().await.remove(line_id);
    ims_register_variant_cache().lock().await.remove(line_id);
    if let Some(gateway) = tun_gateway_cache().lock().await.remove(line_id) {
        gateway.shutdown();
    }
    forget_live_network_overrides(line_id);
}

async fn clear_live_ims_session(line_id: &str, profile: &'static CarrierProfile) {
    clear_live_ims_tcp_channel(line_id, profile).await;

    let profile_id = profile.meta.profile_id;
    let mut ready = ims_register_ready_cache().lock().await;
    if ready
        .get(line_id)
        .is_some_and(|state| state.profile_id == profile_id)
    {
        ready.remove(line_id);
    }
    drop(ready);

    let mut verify = ims_security_verify_cache().lock().await;
    if verify
        .get(line_id)
        .is_some_and(|state| state.profile_id == profile_id)
    {
        verify.remove(line_id);
    }
    drop(verify);

    let mut variant = ims_register_variant_cache().lock().await;
    if variant
        .get(line_id)
        .is_some_and(|state| state.profile_id == profile_id)
    {
        variant.remove(line_id);
    }
}

async fn cached_tun_gateway(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<Arc<TunGatewayRuntime>, LiveStageError> {
    tun_gateway_cache()
        .lock()
        .await
        .get(line_id)
        .filter(|runtime| runtime.is_for_profile(profile.meta.profile_id))
        .cloned()
        .ok_or_else(|| live_stage_error("live_tun_gateway_missing"))
}

pub async fn send_live_sms_over_ims(
    recipient: &str,
    text: &str,
) -> Result<LiveSmsSendResult, LiveStageError> {
    send_live_sms_over_ims_for_line("", recipient, text).await
}

/// Send an IMS SMS using one line's network overrides.
///
/// `line_id` selects that line's ePDG/DNS/proxy settings; an empty value falls
/// back to carrier-profile defaults.
pub async fn send_live_sms_over_ims_for_line(
    line_id: &str,
    recipient: &str,
    text: &str,
) -> Result<LiveSmsSendResult, LiveStageError> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("sms_identity_unavailable"))?;
    // Resolve the carrier from THIS line's SIM, so a second line does not send
    // through the first line's operator profile.
    let identity = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("sms_identity_unavailable"))?;
    let profile_match = resolve_profile_for_line(line_id, identity.imsi.trim())
        .ok_or_else(|| live_stage_error("sms_profile_unmatched"))?;
    let profile = profile_match.profile;

    match tokio::time::timeout(
        LIVE_SMS_SEND_TOTAL_TIMEOUT,
        send_live_sms_over_ims_for_profile(&conn, line_id, profile, recipient, text),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            warn!(
                profile_id = profile.meta.profile_id,
                timeout_ms = LIVE_SMS_SEND_TOTAL_TIMEOUT.as_millis() as u64,
                "VoWiFi SMS send timed out; IMS session cache will be cleared"
            );
            clear_live_ims_session(line_id, profile).await;
            Err(live_stage_error("sms_send_timeout"))
        }
    }
}

async fn send_live_sms_over_ims_for_profile(
    conn: &zbus::Connection,
    line_id: &str,
    profile: &'static CarrierProfile,
    recipient: &str,
    text: &str,
) -> Result<LiveSmsSendResult, LiveStageError> {
    if !cached_live_ims_register_ready(line_id, profile).await {
        info!(
            profile_id = profile.meta.profile_id,
            "VoWiFi SMS send refreshing IMS registration before MESSAGE"
        );
        run_live_ims_register_until(line_id, profile).await?;
    }
    let gateway = cached_tun_gateway(line_id, profile).await?;
    let route = gateway
        .ims_client_tcp_route()
        .map_err(|error| live_stage_error(error.reason()))?;
    if route.profile_id != profile.meta.profile_id {
        return Err(live_stage_error("sms_ims_policy_profile_mismatch"));
    }

    let sim_info = get_sim_info_data_with_cache(conn, None)
        .await
        .map_err(|_| live_stage_error("sms_smsc_unavailable"))?;
    let service_center = sim_info.sms_center.trim();
    if service_center.is_empty() {
        return Err(live_stage_error("sms_smsc_unavailable"));
    }
    let submission = sms::build_single_part_mo_submission(recipient, text, service_center)
        .map_err(|error| live_stage_error(error.to_string()))?;
    let identity =
        live_ims_register_identity(line_id, profile, LiveRegisterIdentityFormat::ImsiHomeDomain)
            .await?;
    let security_verify = cached_live_ims_security_verify(line_id, profile).await;
    let variants = live_sms_request_uri_variants(profile, recipient, service_center)?;

    info!(
        profile_id = profile.meta.profile_id,
        body_bytes = submission.body_bytes,
        text_utf16_units = submission.text_utf16_units,
        part_index = submission.part_index,
        part_count = submission.part_count,
        pcscf_family = ip_family_name(route.remote_addr),
        receiver_transport = profile.sms.receiver_transport,
        security_verify_present = security_verify.is_some(),
        "VoWiFi MO SMS over IMS send prepared"
    );

    match send_live_sms_message_variants(
        line_id,
        profile,
        &route,
        &identity,
        &submission,
        &variants,
        security_verify.as_deref(),
    )
    .await
    {
        Ok(outcome) => Ok(outcome),
        Err(err) if live_sms_session_refresh_retryable(&err.reason) => {
            warn!(
                profile_id = profile.meta.profile_id,
                reason = err.reason.as_str(),
                "VoWiFi MO SMS refreshing IMS session after retryable send failure"
            );
            clear_live_ims_session(line_id, profile).await;
            run_live_ims_register_until(line_id, profile).await?;
            let gateway = cached_tun_gateway(line_id, profile).await?;
            let route = gateway
                .ims_client_tcp_route()
                .map_err(|error| live_stage_error(error.reason()))?;
            let security_verify = cached_live_ims_security_verify(line_id, profile).await;
            send_live_sms_message_variants(
                line_id,
                profile,
                &route,
                &identity,
                &submission,
                &variants,
                security_verify.as_deref(),
            )
            .await
        }
        Err(err) => Err(err),
    }
}

async fn run_register_exchange_over_tunnel(
    line_id: &str,
    profile: &'static CarrierProfile,
    gateway: &TunGatewayRuntime,
) -> Result<String, LiveStageError> {
    let mut last_error = None;
    for pcscf_addr in register_pcscf_candidates(gateway) {
        match run_register_exchange_with_pcscf(line_id, profile, gateway, pcscf_addr).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                warn!(
                    reason = error.reason.as_str(),
                    "IMS REGISTER candidate failed"
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_pcscf_candidate_missing")))
}

fn register_pcscf_candidates(gateway: &TunGatewayRuntime) -> Vec<IpAddr> {
    let mut addrs = Vec::new();
    addrs.push(gateway.pcscf_addr());
    addrs.extend(gateway.pcscf_addrs().iter().copied());
    addrs.sort();
    addrs.dedup();
    addrs
}

async fn run_register_exchange_with_pcscf(
    line_id: &str,
    profile: &'static CarrierProfile,
    gateway: &TunGatewayRuntime,
    pcscf_addr: IpAddr,
) -> Result<String, LiveStageError> {
    let mut last_error = None;
    let variants = live_register_header_variants_for_attempt(line_id, profile).await;
    for variant in variants {
        match run_register_exchange_with_pcscf_variant(
            line_id, profile, gateway, pcscf_addr, variant,
        )
        .await
        {
            Ok(response) => {
                record_live_ims_register_success_variant(line_id, profile, variant).await;
                return Ok(response);
            }
            Err(err) => {
                warn!(
                    register_variant = variant.label,
                    reason = err.reason.as_str(),
                    "IMS REGISTER header variant failed"
                );
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_register_variant_missing")))
}

fn live_register_header_variants(
    profile: &'static CarrierProfile,
) -> &'static [LiveRegisterHeaderVariant] {
    match profile.ims.register.live_header_variant_set {
        "ee_ims_features" => GB_EE_REGISTER_HEADER_VARIANTS,
        _ => LIVE_REGISTER_HEADER_VARIANTS,
    }
}

async fn live_register_header_variants_for_attempt(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Vec<LiveRegisterHeaderVariant> {
    let variants = live_register_header_variants(profile);
    let cached = ims_register_variant_cache()
        .lock()
        .await
        .get(line_id)
        .cloned();
    let Some(cached) = cached.filter(|cached| {
        cached.profile_id == profile.meta.profile_id
            && cached.captured_at.elapsed() <= LIVE_IMS_REGISTER_MAX_TTL
    }) else {
        return variants.to_vec();
    };
    let Some(success) = variants
        .iter()
        .copied()
        .find(|variant| variant.label == cached.label)
    else {
        return variants.to_vec();
    };

    let mut ordered = Vec::with_capacity(variants.len());
    ordered.push(success);
    ordered.extend(
        variants
            .iter()
            .copied()
            .filter(|variant| variant.label != cached.label),
    );
    ordered
}

async fn record_live_ims_register_success_variant(
    line_id: &str,
    profile: &'static CarrierProfile,
    variant: LiveRegisterHeaderVariant,
) {
    ims_register_variant_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsRegisterSuccessVariant {
            profile_id: profile.meta.profile_id,
            label: variant.label,
            captured_at: Instant::now(),
        },
    );
}

async fn run_register_exchange_with_pcscf_variant(
    line_id: &str,
    profile: &'static CarrierProfile,
    gateway: &TunGatewayRuntime,
    pcscf_addr: IpAddr,
    variant: LiveRegisterHeaderVariant,
) -> Result<String, LiveStageError> {
    let target = SocketAddr::new(pcscf_addr, profile.ims.local_port);
    let mut stream =
        connect_tcp_from_inner(gateway.inner_addr(), target, profile.ims.local_port).await?;
    let local_addr = match stream.local_addr() {
        Ok(addr) => addr,
        Err(_) => {
            abort_tcp_stream(stream);
            return Err(live_stage_error("ims_tcp_local_addr_unavailable"));
        }
    };

    match run_register_exchange_on_connected_stream(
        line_id,
        profile,
        &mut stream,
        gateway,
        local_addr,
        pcscf_addr,
        variant,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            abort_tcp_stream(stream);
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_register_exchange_on_connected_stream(
    line_id: &str,
    profile: &'static CarrierProfile,
    stream: &mut TcpStream,
    gateway: &TunGatewayRuntime,
    local_addr: SocketAddr,
    pcscf_addr: IpAddr,
    variant: LiveRegisterHeaderVariant,
) -> Result<String, LiveStageError> {
    let identity = live_ims_register_identity(line_id, profile, variant.identity_format).await?;
    let identity_shape = identity.shape;
    let mut context = LiveRegisterRequestContext::new(profile, identity, local_addr, pcscf_addr)?;
    info!(
        pcscf_family = ip_family_name(pcscf_addr),
        identity_source = profile.ims.identity_source,
        identity_shape = identity_shape,
        register_variant = variant.label,
        route_header_present = variant.include_route_header,
        security_client_format = variant.security_client_format.label(),
        initial_authorization = variant.initial_authorization.label(),
        header_profile = variant.header_profile.contact_features.label(),
        accept_contact_present = variant.header_profile.include_accept_contact,
        p_preferred_identity_present = variant.header_profile.include_p_preferred_identity,
        visited_network_format = variant.header_profile.visited_network.label(),
        pani_format = variant.header_profile.pani.label(),
        cellular_network_info_present = variant.header_profile.include_cellular_network_info,
        user_agent_format = variant.header_profile.user_agent.label(),
        request_uri = variant.request_uri.label(),
        identity_format = variant.identity_format.label(),
        sec_agree_headers_present =
            sec_agree_headers_required(profile, variant.force_sec_agree_headers),
        contact_feature_count = context.contact_feature_count(variant.header_profile),
        local_port = local_addr.port(),
        expected_header_port = profile.ims.local_port,
        sip_instance_present = matches!(
            variant.header_profile.contact_features,
            LiveContactFeatureSet::MmtelSmsSipInstance
        ),
        security_client_present = variant.include_security_client,
        "IMS REGISTER request metadata prepared"
    );
    let request = context.build_initial_request(profile, variant);
    write_sip_request(stream, &request).await?;
    let initial_response = read_sip_response(stream).await?;
    let parsed = ims::parse_sip_response(&initial_response, profile.ims.realm)
        .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
    info!(
        status_code = parsed.status_code,
        reason = parsed.reason.as_str(),
        security_server_offers = parsed.security_server_offers.len(),
        digest_challenge_present = parsed.digest_challenge.is_some(),
        digest_realm_matches_profile = parsed
            .digest_challenge
            .as_ref()
            .is_some_and(|challenge| challenge.realm_matches_profile),
        digest_algorithm = parsed
            .digest_challenge
            .as_ref()
            .map(|challenge| challenge.algorithm.as_str())
            .unwrap_or("absent"),
        warning_present = parsed.warning_present,
        unsupported = ?parsed.unsupported,
        require = ?parsed.require,
        proxy_require = ?parsed.proxy_require,
        "IMS REGISTER initial response metadata received"
    );

    match parsed.status_code {
        200 => Ok(initial_response),
        401 | 407 => {
            let challenge = parse_live_digest_challenge(&initial_response, profile.ims.realm)?;
            reject_plain_digest_when_disabled(profile, &challenge)?;
            info!(
                header_kind = challenge.header_kind,
                algorithm = challenge.algorithm.as_str(),
                qop_present = challenge.qop.is_some(),
                opaque_present = challenge.opaque.is_some(),
                security_server_offer_count = challenge.security_server_offers.len(),
                "IMS REGISTER digest challenge accepted"
            );
            let final_response = run_authenticated_register_after_challenge(
                line_id,
                profile,
                stream,
                gateway,
                &mut context,
                &challenge,
                variant,
            )
            .await?;
            let final_summary = ims::parse_sip_response(&final_response, profile.ims.realm)
                .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
            info!(
                status_code = final_summary.status_code,
                reason = final_summary.reason.as_str(),
                security_server_offers = final_summary.security_server_offers.len(),
                digest_challenge_present = final_summary.digest_challenge.is_some(),
                digest_realm_matches_profile = final_summary
                    .digest_challenge
                    .as_ref()
                    .is_some_and(|challenge| challenge.realm_matches_profile),
                digest_algorithm = final_summary
                    .digest_challenge
                    .as_ref()
                    .map(|challenge| challenge.algorithm.as_str())
                    .unwrap_or("absent"),
                warning_present = final_summary.warning_present,
                unsupported = ?final_summary.unsupported,
                require = ?final_summary.require,
                proxy_require = ?final_summary.proxy_require,
                "IMS REGISTER authenticated response metadata received"
            );
            match final_summary.status_code {
                200 => Ok(final_response),
                401 | 407 => Err(live_stage_error("ims_register_auth_rejected")),
                _ => Err(live_stage_error("ims_register_unexpected_status")),
            }
        }
        _ => Err(live_stage_error("ims_register_initial_unexpected_status")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_authenticated_register_after_challenge(
    line_id: &str,
    profile: &'static CarrierProfile,
    initial_stream: &mut TcpStream,
    gateway: &TunGatewayRuntime,
    context: &mut LiveRegisterRequestContext,
    challenge: &LiveDigestChallenge,
    variant: LiveRegisterHeaderVariant,
) -> Result<String, LiveStageError> {
    let mut challenge = challenge.clone();
    let mut auth_material =
        build_live_register_auth_material(line_id, profile, context, &challenge, variant).await?;
    let mut authenticated_cseq = 2;
    if let Some(auts) = auth_material.auts.take() {
        let resync_authorization = build_digest_resync_authorization_header(
            context,
            &challenge,
            &context.request_uri(profile, variant),
            &auts,
        )?;
        let resync_request =
            context.build_authorized_request(profile, variant, 2, &resync_authorization, None);
        info!("IMS REGISTER AKA resync request ready");
        write_sip_request(initial_stream, &resync_request).await?;
        let resync_response = read_sip_response(initial_stream).await?;
        let resync_summary = ims::parse_sip_response(&resync_response, profile.ims.realm)
            .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
        info!(
            status_code = resync_summary.status_code,
            reason = resync_summary.reason.as_str(),
            digest_challenge_present = resync_summary.digest_challenge.is_some(),
            security_server_offers = resync_summary.security_server_offers.len(),
            "IMS REGISTER AKA resync response metadata received"
        );
        if !matches!(resync_summary.status_code, 401 | 407) {
            return Err(live_stage_error("ims_aka_resync_unexpected_status"));
        }
        let mut resynced_challenge =
            parse_live_digest_challenge(&resync_response, profile.ims.realm)?;
        reject_plain_digest_when_disabled(profile, &resynced_challenge)?;
        if resynced_challenge.security_server_offers.is_empty()
            && !challenge.security_server_offers.is_empty()
        {
            resynced_challenge.security_server_values = challenge.security_server_values.clone();
            resynced_challenge.security_server_offers = challenge.security_server_offers.clone();
        }
        challenge = resynced_challenge;
        auth_material =
            build_live_register_auth_material(line_id, profile, context, &challenge, variant)
                .await?;
        if auth_material.auts.is_some() {
            return Err(live_stage_error("ims_aka_resync_repeated"));
        }
        authenticated_cseq = 3;
    }
    let selected_offer = select_live_security_server_offer(profile, &challenge)?;
    let security_verify = selected_offer.as_ref().map(|offer| offer.raw.clone());
    if let Some(offer) =
        selected_offer.filter(|_| challenge.nonce_kind == LiveDigestNonceKind::AkaChallenge)
    {
        initial_stream
            .shutdown()
            .await
            .map_err(|_| live_stage_error("ims_tcp_shutdown_failed"))?;
        run_protected_authenticated_register_candidates(
            line_id,
            profile,
            gateway,
            context,
            &offer,
            &auth_material,
            variant,
            authenticated_cseq,
            security_verify.as_deref(),
        )
        .await
    } else {
        let authenticated = context.build_authorized_request(
            profile,
            variant,
            authenticated_cseq,
            &auth_material.authorization,
            security_verify.as_deref(),
        );
        write_sip_request(initial_stream, &authenticated).await?;
        read_sip_response(initial_stream).await
    }
}

// These protocol helpers keep security/session inputs explicit. Collapsing them
// into broad context bags would hide which values cross an authentication step.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
async fn run_protected_authenticated_register_candidates(
    line_id: &str,
    profile: &'static CarrierProfile,
    gateway: &TunGatewayRuntime,
    context: &mut LiveRegisterRequestContext,
    offer: &LiveSecurityServerOffer,
    auth_material: &LiveRegisterAuthMaterial,
    variant: LiveRegisterHeaderVariant,
    authenticated_cseq: u32,
    security_verify: Option<&str>,
) -> Result<String, LiveStageError> {
    let local_security = context.security_client_state;
    let mut candidates = Vec::new();
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_primary",
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
    });
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_inverted",
        client_flow_outbound_sa_identifier: offer.spi_c,
        client_flow_inbound_sa_identifier: local_security.spi_s,
        server_flow_outbound_sa_identifier: offer.spi_s,
        server_flow_inbound_sa_identifier: local_security.spi_c,
        secrets: auth_material.ims_esp_secrets.clone(),
    });
    for alt in &auth_material.ims_esp_alt_secrets {
        candidates.push(LiveImsEspPolicyCandidate {
            label: "client_server_flow_primary_raw_ik",
            client_flow_outbound_sa_identifier: offer.spi_s,
            client_flow_inbound_sa_identifier: local_security.spi_c,
            server_flow_outbound_sa_identifier: offer.spi_c,
            server_flow_inbound_sa_identifier: local_security.spi_s,
            secrets: alt.clone(),
        });
    }

    let mut last_error = None;
    for candidate in candidates {
        gateway
            .install_ims_esp_policy(ImsEspPolicyConfig {
                profile_id: profile.meta.profile_id,
                local_addr: gateway.inner_addr(),
                remote_addr: context.route_addr,
                local_port_c: local_security.port_c,
                local_port_s: local_security.port_s,
                remote_port_c: offer.port_c,
                remote_port_s: offer.port_s,
                client_flow: ImsEspFlowConfig {
                    label: "client_flow",
                    local_port: local_security.port_c,
                    remote_port: offer.port_s,
                    outbound_sa_identifier: candidate.client_flow_outbound_sa_identifier,
                    inbound_sa_identifier: candidate.client_flow_inbound_sa_identifier,
                    secrets: candidate.secrets.clone(),
                },
                server_flow: ImsEspFlowConfig {
                    label: "server_flow",
                    local_port: local_security.port_s,
                    remote_port: offer.port_c,
                    outbound_sa_identifier: candidate.server_flow_outbound_sa_identifier,
                    inbound_sa_identifier: candidate.server_flow_inbound_sa_identifier,
                    secrets: candidate.secrets,
                },
            })
            .map_err(|error| live_stage_error(error.reason()))?;
        info!(
            policy_candidate = candidate.label,
            security_verify_present = security_verify.is_some(),
            local_port_c = local_security.port_c,
            local_port_s = local_security.port_s,
            remote_port_c = offer.port_c,
            remote_port_s = offer.port_s,
            "IMS REGISTER will continue over protected ipsec-3gpp transport"
        );
        let target = SocketAddr::new(context.route_addr, offer.port_s);
        match connect_tcp_from_inner(gateway.inner_addr(), target, local_security.port_c).await {
            Ok(mut protected_stream) => {
                let protected_local_addr = protected_stream
                    .local_addr()
                    .map_err(|_| live_stage_error("ims_tcp_local_addr_unavailable"))?;
                context.local_addr = protected_local_addr;
                let authenticated = context.build_authorized_request(
                    profile,
                    variant,
                    authenticated_cseq,
                    &auth_material.authorization,
                    security_verify,
                );
                write_sip_request(&mut protected_stream, &authenticated).await?;
                let response = read_sip_response(&mut protected_stream).await?;
                let summary = ims::parse_sip_response(&response, profile.ims.realm)
                    .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
                if summary.status_code == 200 {
                    record_live_ims_security_verify(
                        line_id,
                        profile,
                        security_verify,
                        summary.expires_seconds,
                    )
                    .await;
                    record_live_ims_tcp_channel(
                        line_id,
                        profile,
                        protected_local_addr,
                        target,
                        protected_stream,
                        security_verify.map(str::to_string),
                        summary.expires_seconds,
                    )
                    .await;
                }
                return Ok(response);
            }
            Err(err) => {
                warn!(
                    policy_candidate = candidate.label,
                    reason = err.reason.as_str(),
                    "IMS protected TCP candidate failed"
                );
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_tcp_connect_failed")))
}

struct LiveImsEspPolicyCandidate {
    label: &'static str,
    client_flow_outbound_sa_identifier: u32,
    client_flow_inbound_sa_identifier: u32,
    server_flow_outbound_sa_identifier: u32,
    server_flow_inbound_sa_identifier: u32,
    secrets: ChildSaSecretPair,
}

#[derive(Debug, Clone)]
struct LiveSmsRequestUriVariant {
    label: &'static str,
    request_uri: String,
    to_uri: String,
}

#[allow(clippy::too_many_arguments)]
async fn send_live_sms_message_variants(
    line_id: &str,
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    submission: &sms::MoSmsSubmission,
    variants: &[LiveSmsRequestUriVariant],
    security_verify: Option<&str>,
) -> Result<LiveSmsSendResult, LiveStageError> {
    let mut last_error = None;
    for variant in variants {
        match send_live_sms_message_variant(
            line_id,
            profile,
            route,
            identity,
            submission,
            variant,
            security_verify,
        )
        .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(err) => {
                let try_next_variant = live_sms_route_variant_retryable(&err.reason);
                warn!(
                    route_variant = variant.label,
                    reason = err.reason.as_str(),
                    try_next_variant,
                    "VoWiFi MO SMS route variant failed"
                );
                last_error = Some(err);
                if !try_next_variant {
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| live_stage_error("sms_message_send_failed")))
}

fn live_sms_route_variant_retryable(reason: &str) -> bool {
    reason.starts_with("sms_message_sip_") || reason == "sip_status_line_invalid"
}

fn live_sms_session_refresh_retryable(reason: &str) -> bool {
    matches!(
        reason,
        "live_tun_gateway_missing"
            | "sms_ims_policy_profile_mismatch"
            | "ims_tcp_socket_failed"
            | "ims_tcp_bind_preferred_port_failed"
            | "ims_tcp_bind_failed"
            | "ims_tcp_connect_timeout"
            | "ims_tcp_connect_failed"
            | "sms_tcp_local_addr_unavailable"
            | "sip_status_line_invalid"
            | "sip_status_line_missing"
            | "sip_status_code_invalid"
            | "sip_frame_empty"
            | "ims_register_initial_unexpected_status"
    ) || matches!(
        reason.strip_prefix("sms_message_sip_"),
        Some("401" | "403" | "407" | "408" | "480" | "481" | "500" | "503")
    )
}

#[allow(clippy::too_many_arguments)]
async fn send_live_sms_message_variant(
    line_id: &str,
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    security_verify: Option<&str>,
) -> Result<LiveSmsSendResult, LiveStageError> {
    if let Some(outcome) = send_live_sms_message_on_cached_channel(
        line_id,
        profile,
        route,
        identity,
        submission,
        variant,
        security_verify,
    )
    .await?
    {
        return Ok(outcome);
    }

    let target = SocketAddr::new(route.remote_addr, route.remote_port);
    let mut stream = connect_tcp_from_inner(route.local_addr, target, route.local_port).await?;
    let mut pending = Vec::new();
    let local_addr = stream
        .local_addr()
        .map_err(|_| live_stage_error("sms_tcp_local_addr_unavailable"))?;
    match send_live_sms_message_on_stream(
        profile,
        route,
        identity,
        submission,
        variant,
        security_verify,
        local_addr,
        &mut stream,
        &mut pending,
    )
    .await
    {
        Ok(outcome) => Ok(start_live_sms_followup_task(
            line_id,
            profile,
            *route,
            identity.clone(),
            submission.clone(),
            variant.clone(),
            security_verify.map(ToString::to_string),
            local_addr,
            stream,
            pending,
            outcome,
        )),
        Err(err) => {
            abort_tcp_stream(stream);
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_live_sms_message_on_cached_channel(
    line_id: &str,
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    security_verify: Option<&str>,
) -> Result<Option<LiveSmsSendResult>, LiveStageError> {
    let channel = {
        let mut guard = ims_tcp_channel_cache().lock().await;
        let Some(channel) = guard.remove(line_id) else {
            return Ok(None);
        };
        channel
    };
    if channel.profile_id != profile.meta.profile_id || channel.expires_at <= Instant::now() {
        let (stream, _) = channel.channel.into_parts();
        abort_tcp_stream(stream);
        return Ok(None);
    }
    let local_addr = crate::connectivity::core::access::ImsChannel::route(&channel.channel).local_addr;
    let (mut stream, mut pending) = channel.channel.into_parts();
    match send_live_sms_message_on_stream(
        profile,
        route,
        identity,
        submission,
        variant,
        security_verify,
        local_addr,
        &mut stream,
        &mut pending,
    )
    .await
    {
        Ok(outcome) => Ok(Some(start_live_sms_followup_task(
            line_id,
            profile,
            *route,
            identity.clone(),
            submission.clone(),
            variant.clone(),
            security_verify.map(ToString::to_string),
            local_addr,
            stream,
            pending,
            outcome,
        ))),
        Err(err) => {
            warn!(
                profile_id = profile.meta.profile_id,
                reason = err.reason.as_str(),
                "VoWiFi cached IMS TCP channel failed during MESSAGE exchange"
            );
            abort_tcp_stream(stream);
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_live_sms_message_on_stream(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
    stream: &mut TcpStream,
    pending: &mut Vec<u8>,
) -> Result<sms::MoSmsSipOutcome, LiveStageError> {
    let request = build_live_sms_message_request(
        profile,
        route,
        identity,
        submission,
        variant,
        security_verify,
        local_addr,
    );
    write_sip_frame(stream, &request).await?;
    let response_frame = read_sip_frame_buffered(
        stream,
        pending,
        LIVE_IMS_REGISTER_READ_TIMEOUT,
        "sms_message_response_timeout",
    )
    .await?;
    let status = parse_sip_status(&response_frame)?;
    info!(
        profile_id = profile.meta.profile_id,
        status_code = status,
        route_variant = variant.label,
        body_bytes = submission.body_bytes,
        "VoWiFi MO SMS SIP MESSAGE response received"
    );
    if !(200..300).contains(&status) {
        return Err(live_stage_error(format!("sms_message_sip_{status}")));
    }

    let outcome = sms::MoSmsSipOutcome {
        trace_id: submission.trace_id.clone(),
        message_id: submission.message_id.clone(),
        sip_status: status,
        rpdu_ack: sms::RpduAckState::None,
        delivery_state: sms::SmsDeliveryState::Accepted,
        failure_cause: None,
        mt_deliveries: Vec::new(),
    };

    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn start_live_sms_followup_task(
    line_id: &str,
    profile: &'static CarrierProfile,
    route: tun_gateway::ImsClientTcpRoute,
    identity: LiveImsRegisterIdentity,
    submission: sms::MoSmsSubmission,
    variant: LiveSmsRequestUriVariant,
    security_verify: Option<String>,
    local_addr: SocketAddr,
    mut stream: TcpStream,
    mut pending: Vec<u8>,
    outcome: sms::MoSmsSipOutcome,
) -> LiveSmsSendResult {
    let (tx, rx) = mpsc::unbounded_channel();
    let followup_seed = outcome.clone();
    let line_id = line_id.to_string();
    tokio::spawn(async move {
        let mut followup_outcome = followup_seed.clone();
        let result = collect_live_sms_followup_frames(
            profile,
            &route,
            &identity,
            &mut stream,
            &submission,
            &variant,
            security_verify.as_deref(),
            local_addr,
            &mut pending,
            &mut followup_outcome,
        )
        .await;

        match result {
            Ok(()) => {
                let _ = tx.send(LiveSmsFollowupFrame {
                    outcome: followup_outcome,
                });
                let expires_at = cached_live_ims_expires_at(&line_id, profile).await;
                let mut guard = ims_tcp_channel_cache().lock().await;
                guard.insert(
                    line_id.clone(),
                    LiveImsTcpChannel {
                        profile_id: profile.meta.profile_id,
                        expires_at,
                        channel: super::channel::EpdgSipChannel::new(
                            stream,
                            pending,
                            shared_vowifi_route(profile, &route, local_addr),
                            security_verify,
                        ),
                    },
                );
            }
            Err(err) => {
                warn!(
                    profile_id = profile.meta.profile_id,
                    reason = err.reason.as_str(),
                    "VoWiFi SMS follow-up task failed; IMS TCP channel discarded"
                );
                abort_tcp_stream(stream);
            }
        }
    });

    LiveSmsSendResult {
        outcome,
        followup: rx,
    }
}

#[allow(clippy::too_many_arguments)]
async fn collect_live_sms_followup_frames(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    stream: &mut TcpStream,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
    pending: &mut Vec<u8>,
    outcome: &mut sms::MoSmsSipOutcome,
) -> Result<(), LiveStageError> {
    let deadline = tokio::time::Instant::now() + LIVE_SMS_FOLLOWUP_WINDOW;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            debug!(
                profile_id = profile.meta.profile_id,
                rpdu_ack = outcome.rpdu_ack.as_str(),
                "VoWiFi SMS follow-up receive window ended"
            );
            return Ok(());
        }
        let timeout = std::cmp::min(
            Duration::from_secs(6),
            deadline.saturating_duration_since(now),
        );
        match read_sip_frame_buffered(stream, pending, timeout, "sms_message_ack_timeout").await {
            Ok(frame) if sip_frame_is_request(&frame, "MESSAGE") => {
                let body = sip_body(&frame);
                let message_kind = classify_sms_followup_body(body);
                let ack = sms::classify_rp_ack(body, submission.rp_message_reference);
                if ack != sms::RpduAckState::None {
                    outcome.rpdu_ack = ack;
                    match outcome.rpdu_ack {
                        sms::RpduAckState::Acked => {
                            outcome.delivery_state = sms::SmsDeliveryState::Accepted;
                        }
                        sms::RpduAckState::Error => {
                            outcome.delivery_state = sms::SmsDeliveryState::Failed;
                            outcome.failure_cause = Some("rp_error".to_string());
                        }
                        sms::RpduAckState::None => {}
                    }
                }
                let mt_deliver = if message_kind == "rp_data_network_to_ms" {
                    match sms::parse_mt_rp_data(body) {
                        Ok(deliver) => Some(deliver),
                        Err(error) => {
                            warn!(
                                profile_id = profile.meta.profile_id,
                                reason = error.to_string(),
                                body_bytes = body.len(),
                                "VoWiFi MT SMS RP-DATA parse failed"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                let response = build_sip_ok_response_for_request(&frame)?;
                write_sip_frame(stream, &response).await?;
                if let Some(deliver) = mt_deliver {
                    let rp_ack_body = sms::build_network_rp_ack(deliver.rp_message_reference);
                    let rp_ack_request = build_live_sms_rp_ack_request(
                        profile,
                        route,
                        identity,
                        variant,
                        &frame,
                        &rp_ack_body,
                        security_verify,
                        local_addr,
                    );
                    write_sip_frame(stream, &rp_ack_request).await?;
                    info!(
                        profile_id = profile.meta.profile_id,
                        body_bytes = rp_ack_body.len(),
                        segment_reference_present = deliver.segment_reference.is_some(),
                        segment_sequence = deliver.segment_sequence,
                        segment_total = deliver.segment_total,
                        "VoWiFi MT SMS RP-ACK MESSAGE sent"
                    );
                    if outcome
                        .mt_deliveries
                        .iter()
                        .any(|existing| existing.is_duplicate_delivery(&deliver))
                    {
                        info!(
                            profile_id = profile.meta.profile_id,
                            segment_reference_present = deliver.segment_reference.is_some(),
                            segment_sequence = deliver.segment_sequence,
                            segment_total = deliver.segment_total,
                            "VoWiFi MT SMS duplicate delivery suppressed"
                        );
                    } else {
                        outcome.mt_deliveries.push(deliver);
                    }
                }
                info!(
                    profile_id = profile.meta.profile_id,
                    rpdu_ack = outcome.rpdu_ack.as_str(),
                    body_bytes = body.len(),
                    message_kind = message_kind,
                    mt_delivery_count = outcome.mt_deliveries.len(),
                    "VoWiFi SMS network MESSAGE processed"
                );
                if outcome.rpdu_ack != sms::RpduAckState::None {
                    return Ok(());
                }
            }
            Ok(frame) => {
                if let Ok(status) = parse_sip_status(&frame) {
                    info!(
                        profile_id = profile.meta.profile_id,
                        status_code = status,
                        frame_bytes = frame.len(),
                        "VoWiFi SMS follow-up SIP response received"
                    );
                } else {
                    debug!(
                        profile_id = profile.meta.profile_id,
                        frame_bytes = frame.len(),
                        "VoWiFi SMS received non-MESSAGE frame after SIP 2xx"
                    );
                }
            }
            Err(err) if err.reason == "sms_message_ack_timeout" => {
                debug!(
                    profile_id = profile.meta.profile_id,
                    rpdu_ack = outcome.rpdu_ack.as_str(),
                    "VoWiFi SMS follow-up frame timeout"
                );
                return Ok(());
            }
            Err(err) => return Err(err),
        }
    }
}

fn classify_sms_followup_body(body: &[u8]) -> &'static str {
    match body.first().copied() {
        Some(0x01) => "rp_data_network_to_ms",
        Some(0x03) => "rp_ack_ms_to_network",
        Some(0x04) => "rp_ack_network_to_ms",
        Some(0x05) => "rp_error_ms_to_network",
        Some(0x06) => "rp_error_network_to_ms",
        _ => "unknown",
    }
}

fn build_live_sms_message_request(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let route_host = sip_host(route.remote_addr);
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let to_value = format!("<{}>", variant.to_uri);
    let mut headers = vec![
        crate::connectivity::core::sip_message::SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", profile.ims.local_port),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Preferred-Identity",
            format!("<{}>", identity.public_uri),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Access-Network-Info",
            build_p_access_network_info(profile),
        ),
    ];
    if let Some(security_verify) = security_verify {
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Security-Verify",
            security_verify,
        ));
    }
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Accept-Contact",
        "*;+g.3gpp.smsip",
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "User-Agent",
        build_live_user_agent(profile, LiveUserAgentFormat::ProfileDefault),
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Content-Type",
        "application/vnd.3gpp.sms",
    ));
    crate::connectivity::core::sip_message::build_message(&crate::connectivity::core::sip_message::SipRequest {
        method: "MESSAGE",
        request_uri: &variant.request_uri,
        route: shared_vowifi_route(profile, route, local_addr),
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &call_id,
        cseq: 1,
        headers: &headers,
        body: &submission.body,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_live_sms_rp_ack_request(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    variant: &LiveSmsRequestUriVariant,
    inbound_frame: &[u8],
    body: &[u8],
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let route_host = sip_host(route.remote_addr);
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let request_uri =
        sip_header_uri(inbound_frame, "From").unwrap_or_else(|| variant.request_uri.clone());
    let to_value = format!("<{request_uri}>");
    let mut headers = vec![
        crate::connectivity::core::sip_message::SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", profile.ims.local_port),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Preferred-Identity",
            format!("<{}>", identity.public_uri),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Access-Network-Info",
            build_p_access_network_info(profile),
        ),
    ];
    if let Some(security_verify) = security_verify {
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Security-Verify",
            security_verify,
        ));
    }
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Accept-Contact",
        "*;+g.3gpp.smsip",
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "User-Agent",
        build_live_user_agent(profile, LiveUserAgentFormat::ProfileDefault),
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Content-Type",
        "application/vnd.3gpp.sms",
    ));
    crate::connectivity::core::sip_message::build_rp_ack(&crate::connectivity::core::sip_message::SipRequest {
        method: "MESSAGE",
        request_uri: &request_uri,
        route: shared_vowifi_route(profile, route, local_addr),
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &call_id,
        cseq: 1,
        headers: &headers,
        body,
    })
}

fn shared_vowifi_route(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    local_addr: SocketAddr,
) -> crate::connectivity::core::context::ImsRoute {
    crate::connectivity::core::context::ImsRoute {
        local_addr: SocketAddr::new(local_addr.ip(), profile.ims.local_port),
        pcscf_addr: SocketAddr::new(route.remote_addr, profile.ims.local_port),
        transport: crate::connectivity::core::context::SipTransport::Tcp,
    }
}

fn build_live_invite_request(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    invite: &voice::MoCallInvite,
    request_uri: &str,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let route_host = sip_host(route.remote_addr);
    let from_tag = hex_token(8);
    let to_value = format!("<{request_uri}>");
    let mut headers = vec![
        crate::connectivity::core::sip_message::SipHeader::new(
            "Route",
            format!("<sip:{route_host}:{};lr>", profile.ims.local_port),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "Contact",
            format!(
                "<{}>;+g.3gpp.icsi-ref=\"{}\"",
                identity.public_uri, LIVE_VOICE_MMTEL_ICSI
            ),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Preferred-Identity",
            format!("<{}>", identity.public_uri),
        ),
        crate::connectivity::core::sip_message::SipHeader::new(
            "P-Access-Network-Info",
            build_p_access_network_info(profile),
        ),
    ];
    if let Some(security_verify) = security_verify {
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Security-Verify",
            security_verify,
        ));
    }
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Accept-Contact",
        format!("*;+g.3gpp.icsi-ref=\"{LIVE_VOICE_MMTEL_ICSI}\""),
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "P-Asserted-Service",
        "urn:urn-7:3gpp-service.ims.icsi.mmtel",
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Supported",
        "100rel,timer",
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Allow",
        "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK",
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "User-Agent",
        build_live_user_agent(profile, LiveUserAgentFormat::ProfileDefault),
    ));
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "Content-Type",
        "application/sdp",
    ));
    crate::connectivity::core::sip_message::build_invite(&crate::connectivity::core::sip_message::SipRequest {
        method: "INVITE",
        request_uri,
        route: shared_vowifi_route(profile, route, local_addr),
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &invite.call_id,
        cseq: 1,
        headers: &headers,
        body: &invite.sdp_offer,
    })
}

fn build_live_ack_request(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    invite: &voice::MoCallInvite,
    request_uri: &str,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let route_host = sip_host(route.remote_addr);
    let from_tag = hex_token(8);
    let to_value = format!("<{request_uri}>");
    let mut headers = vec![crate::connectivity::core::sip_message::SipHeader::new(
        "Route",
        format!("<sip:{route_host}:{};lr>", profile.ims.local_port),
    )];
    if let Some(security_verify) = security_verify {
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Security-Verify",
            security_verify,
        ));
    }
    headers.push(crate::connectivity::core::sip_message::SipHeader::new(
        "User-Agent",
        build_live_user_agent(profile, LiveUserAgentFormat::ProfileDefault),
    ));
    crate::connectivity::core::sip_message::build_ack(&crate::connectivity::core::sip_message::SipRequest {
        method: "ACK",
        request_uri,
        route: shared_vowifi_route(profile, route, local_addr),
        branch: &branch,
        from_uri: &identity.public_uri,
        from_tag: &from_tag,
        to_value: &to_value,
        call_id: &invite.call_id,
        cseq: 1,
        headers: &headers,
        body: &[],
    })
}

fn sip_header_uri(frame: &[u8], header_name: &str) -> Option<String> {
    let header_end = find_sip_header_end(frame)?;
    let headers = std::str::from_utf8(&frame[..header_end]).ok()?;
    sip_header_values(headers, header_name)
        .into_iter()
        .find_map(|value| sip_uri_from_header_value(&value))
}

fn sip_uri_from_header_value(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(start) = value.find('<') {
        let rest = &value[start + 1..];
        let end = rest.find('>')?;
        let uri = rest[..end].trim();
        return sip_uri_is_supported(uri).then(|| uri.to_string());
    }

    let uri = value
        .split(';')
        .next()
        .unwrap_or_default()
        .split(',')
        .next()
        .unwrap_or_default()
        .trim();
    sip_uri_is_supported(uri).then(|| uri.to_string())
}

fn sip_uri_is_supported(uri: &str) -> bool {
    uri.starts_with("sip:") || uri.starts_with("sips:") || uri.starts_with("tel:")
}

fn live_sms_request_uri_variants(
    profile: &'static CarrierProfile,
    recipient: &str,
    service_center: &str,
) -> Result<Vec<LiveSmsRequestUriVariant>, LiveStageError> {
    let recipient_user = sip_phone_user(recipient)?;
    let service_center_user = sip_phone_user(service_center)?;
    let to_uri = format!("sip:{recipient_user}@{};user=phone", profile.ims.domain);
    Ok(vec![
        LiveSmsRequestUriVariant {
            label: "service_center_sip_user_phone",
            request_uri: format!(
                "sip:{service_center_user}@{};user=phone",
                profile.ims.domain
            ),
            to_uri: to_uri.clone(),
        },
        LiveSmsRequestUriVariant {
            label: "service_center_tel",
            request_uri: format!("tel:{service_center_user}"),
            to_uri: to_uri.clone(),
        },
        LiveSmsRequestUriVariant {
            label: "recipient_sip_user_phone",
            request_uri: to_uri.clone(),
            to_uri,
        },
    ])
}

fn sip_phone_user(value: &str) -> Result<String, LiveStageError> {
    let trimmed = value.trim();
    let mut out = String::new();
    for (index, ch) in trimmed.chars().enumerate() {
        match ch {
            '+' if index == 0 => out.push(ch),
            '0'..='9' => out.push(ch),
            ' ' | '-' | '(' | ')' => {}
            _ => return Err(live_stage_error("sms_phone_uri_invalid")),
        }
    }
    if out.is_empty() || out == "+" || out.trim_start_matches('+').len() > 20 {
        return Err(live_stage_error("sms_phone_uri_invalid"));
    }
    Ok(out)
}

async fn write_sip_request(stream: &mut TcpStream, request: &str) -> Result<(), LiveStageError> {
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| live_stage_error("ims_register_write_failed"))
}

async fn write_sip_frame(stream: &mut TcpStream, frame: &[u8]) -> Result<(), LiveStageError> {
    stream
        .write_all(frame)
        .await
        .map_err(|_| live_stage_error("sms_message_write_failed"))
}

async fn connect_tcp_from_inner(
    inner_addr: IpAddr,
    target: SocketAddr,
    preferred_local_port: u16,
) -> Result<TcpStream, LiveStageError> {
    let socket = match target {
        SocketAddr::V4(_) => TcpSocket::new_v4(),
        SocketAddr::V6(_) => TcpSocket::new_v6(),
    }
    .map_err(|_| live_stage_error("ims_tcp_socket_failed"))?;
    let _ = socket.set_reuseaddr(true);
    if preferred_local_port != 0 {
        socket
            .bind(SocketAddr::new(inner_addr, preferred_local_port))
            .map_err(|_| live_stage_error("ims_tcp_bind_preferred_port_failed"))?;
    } else {
        socket
            .bind(SocketAddr::new(inner_addr, 0))
            .map_err(|_| live_stage_error("ims_tcp_bind_failed"))?;
    }
    tokio::time::timeout(LIVE_IMS_TCP_TIMEOUT, socket.connect(target))
        .await
        .map_err(|_| live_stage_error("ims_tcp_connect_timeout"))?
        .map_err(|_| live_stage_error("ims_tcp_connect_failed"))
}

fn abort_tcp_stream(stream: TcpStream) {
    #[cfg(unix)]
    {
        use std::mem;
        use std::os::fd::AsRawFd;

        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        unsafe {
            let _ = libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                &linger as *const _ as *const libc::c_void,
                mem::size_of::<libc::linger>() as libc::socklen_t,
            );
        }
    }
    drop(stream);
}

async fn read_sip_response(stream: &mut TcpStream) -> Result<String, LiveStageError> {
    let buffer = read_sip_frame(
        stream,
        LIVE_IMS_REGISTER_READ_TIMEOUT,
        "ims_register_read_timeout",
    )
    .await?;
    String::from_utf8(buffer).map_err(|_| live_stage_error("ims_register_response_not_utf8"))
}

async fn read_sip_frame(
    stream: &mut TcpStream,
    timeout: Duration,
    timeout_reason: &'static str,
) -> Result<Vec<u8>, LiveStageError> {
    let mut buffer = Vec::with_capacity(4096);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        let mut chunk = [0u8; 1024];
        tokio::select! {
            _ = &mut deadline => return Err(live_stage_error(timeout_reason)),
            read = stream.read(&mut chunk) => {
                let read = read.map_err(|_| live_stage_error("ims_register_read_failed"))?;
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if sip_message_complete(&buffer) {
                    break;
                }
                if buffer.len() > 16 * 1024 {
                    return Err(live_stage_error("ims_register_response_too_large"));
                }
            }
        }
    }
    Ok(buffer)
}

async fn read_sip_frame_buffered(
    stream: &mut TcpStream,
    pending: &mut Vec<u8>,
    timeout: Duration,
    timeout_reason: &'static str,
) -> Result<Vec<u8>, LiveStageError> {
    if let Some(frame_len) = sip_complete_frame_len(pending) {
        return Ok(pending.drain(..frame_len).collect());
    }

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        let mut chunk = [0u8; 1024];
        tokio::select! {
            _ = &mut deadline => return Err(live_stage_error(timeout_reason)),
            read = stream.read(&mut chunk) => {
                let read = read.map_err(|_| live_stage_error("ims_register_read_failed"))?;
                if read == 0 {
                    break;
                }
                pending.extend_from_slice(&chunk[..read]);
                if pending.len() > 64 * 1024 {
                    return Err(live_stage_error("sip_frame_buffer_too_large"));
                }
                if let Some(frame_len) = sip_complete_frame_len(pending) {
                    return Ok(pending.drain(..frame_len).collect());
                }
            }
        }
    }

    if pending.is_empty() {
        Err(live_stage_error("sip_frame_empty"))
    } else {
        Ok(std::mem::take(pending))
    }
}

fn parse_sip_status(frame: &[u8]) -> Result<u16, LiveStageError> {
    // Shared IMS framing core; remap its neutral reason to the live code.
    crate::connectivity::core::sip_frame::parse_status(frame).map_err(|err| live_stage_error(err.code()))
}

fn sip_frame_is_request(frame: &[u8], method: &str) -> bool {
    frame.starts_with(method.as_bytes()) && frame.get(method.len()) == Some(&b' ')
}

fn sip_body(frame: &[u8]) -> &[u8] {
    crate::connectivity::core::sip_frame::body(frame)
}

fn build_sip_ok_response_for_request(frame: &[u8]) -> Result<Vec<u8>, LiveStageError> {
    let header_end =
        find_sip_header_end(frame).ok_or_else(|| live_stage_error("sip_header_missing"))?;
    let headers = std::str::from_utf8(&frame[..header_end])
        .map_err(|_| live_stage_error("sip_header_not_utf8"))?;
    let mut response = String::from("SIP/2.0 200 OK\r\n");
    append_sip_header_values(&mut response, headers, "Via");
    append_sip_header_values(&mut response, headers, "From");
    append_sip_header_values(&mut response, headers, "To");
    append_sip_header_values(&mut response, headers, "Call-ID");
    append_sip_header_values(&mut response, headers, "CSeq");
    response.push_str("Content-Length: 0\r\n\r\n");
    Ok(response.into_bytes())
}

fn append_sip_header_values(out: &mut String, headers: &str, name: &str) {
    for line in headers.lines() {
        let Some((header_name, value)) = line.split_once(':') else {
            continue;
        };
        if header_name.eq_ignore_ascii_case(name) {
            out.push_str(name);
            out.push(':');
            out.push_str(value);
            if name.eq_ignore_ascii_case("To") && !value.to_ascii_lowercase().contains(";tag=") {
                out.push_str(";tag=");
                out.push_str(&hex_token(8));
            }
            out.push_str("\r\n");
        }
    }
}

fn sip_message_complete(buffer: &[u8]) -> bool {
    sip_complete_frame_len(buffer).is_some()
}

fn sip_complete_frame_len(buffer: &[u8]) -> Option<usize> {
    crate::connectivity::core::sip_frame::complete_frame_len(buffer)
}

fn find_sip_header_end(buffer: &[u8]) -> Option<usize> {
    crate::connectivity::core::sip_frame::find_header_end(buffer)
}

#[derive(Debug, Clone)]
struct LiveImsRegisterIdentity {
    shared: crate::connectivity::core::context::ImsIdentity,
    shape: &'static str,
}

impl std::ops::Deref for LiveImsRegisterIdentity {
    type Target = crate::connectivity::core::context::ImsIdentity;

    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

struct LiveRegisterRequestContext {
    identity: LiveImsRegisterIdentity,
    local_addr: SocketAddr,
    route_addr: IpAddr,
    from_tag: String,
    call_id: String,
    instance_id: String,
    security_client_state: LiveSecurityClientState,
    security_client_full_spaced: String,
    security_client_full_compact: String,
    security_client_minimal_spaced: String,
}

impl LiveRegisterRequestContext {
    fn new(
        profile: &'static CarrierProfile,
        identity: LiveImsRegisterIdentity,
        local_addr: SocketAddr,
        route_addr: IpAddr,
    ) -> Result<Self, LiveStageError> {
        let security_client_state = LiveSecurityClientState::new(live_runtime_config())?;
        Ok(Self {
            identity,
            local_addr,
            route_addr,
            from_tag: hex_token(8),
            call_id: format!("{}@simadmin", hex_token(16)),
            instance_id: format_sip_instance_id(profile)?,
            security_client_state,
            security_client_full_spaced: build_security_client_header(
                profile,
                LiveSecurityClientFormat::FullSpaced,
                &security_client_state,
            ),
            security_client_full_compact: build_security_client_header(
                profile,
                LiveSecurityClientFormat::FullCompact,
                &security_client_state,
            ),
            security_client_minimal_spaced: build_security_client_header(
                profile,
                LiveSecurityClientFormat::MinimalSpaced,
                &security_client_state,
            ),
        })
    }

    fn build_initial_request(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
    ) -> String {
        self.build_register_request(profile, variant, 1, None, None)
    }

    fn build_authenticated_request(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
        authorization: &str,
        security_verify: Option<&str>,
    ) -> String {
        self.build_authorized_request(profile, variant, 2, authorization, security_verify)
    }

    fn build_authorized_request(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
        cseq: u32,
        authorization: &str,
        security_verify: Option<&str>,
    ) -> String {
        self.build_register_request(profile, variant, cseq, Some(authorization), security_verify)
    }

    fn build_register_request(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
        cseq: u32,
        authorization: Option<&str>,
        security_verify: Option<&str>,
    ) -> String {
        let branch = format!("z9hG4bK{}", hex_token(12));
        let request_uri = self.request_uri(profile, variant);
        let local_host = sip_host(self.local_addr.ip());
        let visited_network = format!(
            "ims.mnc{}.mcc{}.3gppnetwork.org",
            three_digit_mnc(profile),
            profile.meta.mcc
        );
        let require_sec_agree =
            sec_agree_headers_required(profile, variant.force_sec_agree_headers);
        let pani = match variant.header_profile.pani {
            LivePaniFormat::ProfileDefault => {
                Some(build_p_access_network_info(profile).to_string())
            }
            LivePaniFormat::PlainWifi => {
                Some("IEEE-802.11;i-wlan-node-id=000000000000".to_string())
            }
            LivePaniFormat::Omit => None,
        };
        let visited_network_header = match variant.header_profile.visited_network {
            LiveVisitedNetworkFormat::QuotedHome => Some(format!("\"{visited_network}\"")),
            LiveVisitedNetworkFormat::UnquotedHome => Some(visited_network),
            LiveVisitedNetworkFormat::Omit => None,
        };
        let params = crate::connectivity::core::context::ImsRegisterParams {
            realm: profile.ims.realm.to_string(),
            domain: profile.ims.domain.to_string(),
            registrar: profile.ims.registrar.map(str::to_string),
            supported_header: profile.ims.register.supported_header.to_string(),
            require_sec_agree,
            user_agent: build_live_user_agent(profile, variant.header_profile.user_agent),
            pani,
            visited_network: visited_network_header,
            allow_header: "INVITE,ACK,CANCEL,BYE,UPDATE,PRACK,MESSAGE,REFER,NOTIFY,INFO,OPTIONS"
                .to_string(),
            // Carrier-configurable: some networks reject the common 3600 default.
            expires: profile.ims.register.expires_seconds,
        };
        let mut headers = Vec::new();
        let authorization = authorization.map(str::to_string).or_else(|| {
            (cseq == 1)
                .then(|| self.build_initial_authorization_header(profile, variant))
                .flatten()
        });
        if let Some(authorization) = authorization {
            if let Some((name, value)) = authorization.split_once(':') {
                headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                    name.trim(),
                    value.trim(),
                ));
            }
        }
        let contact = self.build_contact_header(&local_host, variant.header_profile);
        if let Some(value) = contact
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("Contact: ")
        {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new("Contact", value));
        }
        if variant.header_profile.include_accept_contact {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Accept-Contact",
                "*;+g.3gpp.smsip",
            ));
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Accept-Contact",
                format!("*;+g.3gpp.icsi-ref=\"{IMS_MMTEL_ICSI_REF}\""),
            ));
        }
        if variant.include_route_header {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Route",
                format!(
                    "<sip:{}:{};lr>",
                    sip_host(self.route_addr),
                    profile.ims.local_port
                ),
            ));
        }
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Expires",
            params.expires.to_string(),
        ));
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Supported",
            &params.supported_header,
        ));
        if params.require_sec_agree {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Require",
                "sec-agree",
            ));
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Proxy-Require",
                "sec-agree",
            ));
        }
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "Allow",
            &params.allow_header,
        ));
        if variant.header_profile.include_p_preferred_identity {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "P-Preferred-Identity",
                format!("<{}>", self.identity.public_uri),
            ));
        }
        if let Some(visited_network) = &params.visited_network {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "P-Visited-Network-ID",
                visited_network,
            ));
        }
        if let Some(pani) = &params.pani {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "P-Access-Network-Info",
                pani,
            ));
        }
        if variant.header_profile.include_cellular_network_info {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Cellular-Network-Info",
                build_cellular_network_info(profile),
            ));
        }
        if variant.include_security_client {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Security-Client",
                self.security_client_header(variant.security_client_format),
            ));
        }
        if let Some(security_verify) = security_verify {
            headers.push(crate::connectivity::core::sip_message::SipHeader::new(
                "Security-Verify",
                security_verify,
            ));
        }
        headers.push(crate::connectivity::core::sip_message::SipHeader::new(
            "User-Agent",
            &params.user_agent,
        ));
        let to_value = format!("<{}>", self.identity.public_uri);
        let frame = crate::connectivity::core::sip_message::build_register(&crate::connectivity::core::sip_message::SipRequest {
            method: "REGISTER",
            request_uri: &request_uri,
            route: crate::connectivity::core::context::ImsRoute {
                local_addr: SocketAddr::new(self.local_addr.ip(), profile.ims.local_port),
                pcscf_addr: SocketAddr::new(self.route_addr, profile.ims.local_port),
                transport: crate::connectivity::core::context::SipTransport::Tcp,
            },
            branch: &branch,
            from_uri: &self.identity.public_uri,
            from_tag: &self.from_tag,
            to_value: &to_value,
            call_id: &self.call_id,
            cseq,
            headers: &headers,
            body: &[],
        });
        String::from_utf8(frame).expect("REGISTER builder emits UTF-8 headers")
    }

    fn request_uri(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
    ) -> String {
        match variant.request_uri {
            LiveRegisterRequestUri::HomeRegistrar => {
                let route_domain = profile.ims.registrar.unwrap_or(profile.ims.domain);
                format!("sip:{route_domain}")
            }
            LiveRegisterRequestUri::PcscfSocket => {
                format!(
                    "sip:{}:{}",
                    sip_host(self.route_addr),
                    profile.ims.local_port
                )
            }
        }
    }

    fn build_initial_authorization_header(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
    ) -> Option<String> {
        match variant.initial_authorization {
            LiveInitialAuthorizationFormat::None => None,
            LiveInitialAuthorizationFormat::AkaEmpty => Some(format!(
                "Authorization: Digest username=\"{}\",realm=\"{}\",nonce=\"\",uri=\"{}\",response=\"\",algorithm=AKAv1-MD5",
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(profile.ims.realm),
                quote_sip_param(&self.request_uri(profile, variant))
            )),
            LiveInitialAuthorizationFormat::AkaEmptyUriFirst => Some(format!(
                "Authorization: Digest uri=\"{}\",username=\"{}\",algorithm=AKAv1-MD5,response=\"\",realm=\"{}\",nonce=\"\"",
                quote_sip_param(&self.request_uri(profile, variant)),
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(profile.ims.realm)
            )),
            LiveInitialAuthorizationFormat::AkaEmptyUriFirstNoAlgorithm => Some(format!(
                "Authorization: Digest uri=\"{}\",username=\"{}\",response=\"\",realm=\"{}\",nonce=\"\"",
                quote_sip_param(&self.request_uri(profile, variant)),
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(profile.ims.realm)
            )),
            LiveInitialAuthorizationFormat::AkaZeroResponse => Some(format!(
                "Authorization: Digest username=\"{}\",realm=\"{}\",nonce=\"\",uri=\"{}\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(profile.ims.realm),
                quote_sip_param(&self.request_uri(profile, variant))
            )),
            LiveInitialAuthorizationFormat::AkaZeroResponseUriFirst => Some(format!(
                "Authorization: Digest uri=\"{}\",username=\"{}\",algorithm=AKAv1-MD5,response=\"00000000000000000000000000000000\",realm=\"{}\",nonce=\"\"",
                quote_sip_param(&self.request_uri(profile, variant)),
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(profile.ims.realm)
            )),
        }
    }

    fn security_client_header(&self, format: LiveSecurityClientFormat) -> &str {
        match format {
            LiveSecurityClientFormat::FullSpaced => &self.security_client_full_spaced,
            LiveSecurityClientFormat::FullCompact => &self.security_client_full_compact,
            LiveSecurityClientFormat::MinimalSpaced => &self.security_client_minimal_spaced,
        }
    }

    fn contact_feature_count(&self, header_profile: LiveRegisterHeaderProfile) -> usize {
        match header_profile.contact_features {
            LiveContactFeatureSet::SmsOnly => 2,
            LiveContactFeatureSet::MmtelSmsSipInstance => 5,
        }
    }

    fn build_contact_header(
        &self,
        local_host: &str,
        header_profile: LiveRegisterHeaderProfile,
    ) -> String {
        let contact_port = 5060;
        let user_phone = if self.identity.contact_user_phone {
            ";user=phone"
        } else {
            ""
        };
        let mut header = format!(
            "Contact: <sip:{}@{}:{}{};transport=tcp>",
            self.identity.contact_user, local_host, contact_port, user_phone
        );
        match header_profile.contact_features {
            LiveContactFeatureSet::SmsOnly => {
                header.push_str(";+g.3gpp.accesstype=\"IEEE-802.11\"");
                header.push_str(";+g.3gpp.smsip");
            }
            LiveContactFeatureSet::MmtelSmsSipInstance => {
                header.push_str(";+g.3gpp.accesstype=\"IEEE-802.11\"");
                header.push_str(";audio");
                header.push_str(";+g.3gpp.smsip");
                header.push_str(&format!(";+g.3gpp.icsi-ref=\"{}\"", IMS_MMTEL_ICSI_REF));
                header.push_str(&format!(";+sip.instance=\"<{}>\"", self.instance_id));
            }
        }
        header.push_str(";expires=3600\r\n");
        header
    }
}

#[derive(Debug, Clone, Copy)]
struct LiveSecurityClientState {
    spi_c: u32,
    spi_s: u32,
    port_c: u16,
    port_s: u16,
}

impl LiveSecurityClientState {
    fn new(config: LiveRuntimeConfig) -> Result<Self, LiveStageError> {
        Ok(Self {
            spi_c: random_u32_nonzero()?,
            spi_s: random_u32_nonzero()?,
            port_c: config.ims_security_port_c,
            port_s: config.ims_security_port_s,
        })
    }
}

async fn live_ims_register_identity(
    line_id: &str,
    profile: &'static CarrierProfile,
    format: LiveRegisterIdentityFormat,
) -> Result<LiveImsRegisterIdentity, LiveStageError> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("ims_identity_unavailable"))?;
    // Register with THIS line's IMSI; the global lookup would present the first
    // modem's subscriber for every line.
    let sim = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("ims_identity_unavailable"))?;
    let imsi = sim.imsi.trim();
    if imsi.is_empty()
        || imsi.len() < 5
        || imsi.len() > 16
        || !imsi.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(live_stage_error("ims_identity_unavailable"));
    }
    if !imsi.starts_with(profile.meta.plmn) {
        return Err(live_stage_error("ims_identity_profile_mismatch"));
    }

    Ok(match format {
        LiveRegisterIdentityFormat::ImsiHomeDomain => LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: format!("{imsi}@{}", profile.ims.realm),
                public_uri: format!("sip:{imsi}@{}", profile.ims.domain),
                contact_user: imsi.to_string(),
                home_domain: profile.ims.domain.to_string(),
                contact_user_phone: false,
            },
            shape: "imsi_home_domain",
        },
        LiveRegisterIdentityFormat::PrefixedImsiHomeDomain => {
            let prefixed = format!("0{imsi}");
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: format!("{prefixed}@{}", profile.ims.realm),
                    public_uri: format!("sip:{prefixed}@{}", profile.ims.domain),
                    contact_user: prefixed,
                    home_domain: profile.ims.domain.to_string(),
                    contact_user_phone: false,
                },
                shape: "prefixed_imsi_home_domain",
            }
        }
        LiveRegisterIdentityFormat::ImsiPhoneUri => LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: format!("{imsi}@{}", profile.ims.realm),
                public_uri: format!("sip:{imsi}@{};user=phone", profile.ims.domain),
                contact_user: imsi.to_string(),
                home_domain: profile.ims.domain.to_string(),
                contact_user_phone: true,
            },
            shape: "imsi_phone_uri",
        },
        LiveRegisterIdentityFormat::MsisdnPhoneUri => {
            let phone_number = read_live_msisdn_candidate(&conn).await?;
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: format!("{imsi}@{}", profile.ims.realm),
                    public_uri: format!("sip:{}@{};user=phone", phone_number, profile.ims.domain),
                    contact_user: phone_number,
                    home_domain: profile.ims.domain.to_string(),
                    contact_user_phone: true,
                },
                shape: "msisdn_phone_uri",
            }
        }
    })
}

async fn read_live_msisdn_candidate(conn: &zbus::Connection) -> Result<String, LiveStageError> {
    let info = get_sim_info_data_with_cache(conn, None)
        .await
        .map_err(|_| live_stage_error("ims_msisdn_unavailable"))?;
    let Some(number) = info.phone_numbers.into_iter().find(|number| {
        let digits = number.trim_start_matches('+');
        !digits.is_empty()
            && digits.len() >= 8
            && digits.len() <= 18
            && digits.chars().all(|ch| ch.is_ascii_digit())
    }) else {
        return Err(live_stage_error("ims_msisdn_unavailable"));
    };
    info!(
        msisdn_present = true,
        phone_digits_len = number.trim_start_matches('+').len(),
        "IMS public identity MSISDN candidate prepared"
    );
    Ok(number)
}

#[derive(Clone)]
struct LiveDigestChallenge {
    header_kind: &'static str,
    realm: String,
    nonce: String,
    algorithm: String,
    qop: Option<&'static str>,
    opaque: Option<String>,
    rand: Vec<u8>,
    autn: Vec<u8>,
    nonce_kind: LiveDigestNonceKind,
    security_server_values: Vec<String>,
    security_server_offers: Vec<LiveSecurityServerOffer>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LiveSecurityServerOffer {
    raw: String,
    alg: String,
    ealg: String,
    protocol: String,
    mode: String,
    spi_c: u32,
    spi_s: u32,
    port_c: u16,
    port_s: u16,
    q_milli: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LiveDigestNonceKind {
    AkaChallenge,
    PlainDigest,
}

impl LiveDigestNonceKind {
    fn label(self) -> &'static str {
        match self {
            Self::AkaChallenge => "aka_challenge",
            Self::PlainDigest => "plain_digest",
        }
    }
}

struct LiveRegisterAuthMaterial {
    authorization: String,
    ims_esp_secrets: ChildSaSecretPair,
    ims_esp_alt_secrets: Vec<ChildSaSecretPair>,
    auts: Option<Vec<u8>>,
}

async fn build_live_register_auth_material(
    line_id: &str,
    profile: &'static CarrierProfile,
    context: &LiveRegisterRequestContext,
    challenge: &LiveDigestChallenge,
    variant: LiveRegisterHeaderVariant,
) -> Result<LiveRegisterAuthMaterial, LiveStageError> {
    let digest_uri = context.request_uri(profile, variant);
    let cnonce = live_digest_cnonce()?;
    let (response, ims_esp_secrets, ims_esp_alt_secrets) = match challenge.nonce_kind {
        LiveDigestNonceKind::AkaChallenge => {
            let rand = challenge.rand.clone();
            let autn = challenge.autn.clone();
            let proxy_socket = live_runtime_config().qmi_proxy_socket;
            // AKA must run against this line's own reader, never the global
            // fallback device: a wrong card returns a valid-looking RES that the
            // network then rejects.
            let sim_device = sim_device_for_line(line_id);
            let aka_result = tokio::task::spawn_blocking(move || {
                execute_usim_authenticate_via_proxy_reason_with_retry(
                    proxy_socket.as_str(),
                    sim_device.qmi_device.as_str(),
                    sim_device.uim_slot,
                    USIM_AID_PREFIX,
                    &rand,
                    &autn,
                    LIVE_SIM_AUTH_ATTEMPTS,
                    LIVE_SIM_AUTH_TIMEOUT,
                    LIVE_SIM_AUTH_RETRY_DELAY,
                )
            })
            .await
            .map_err(|_| live_stage_error("ims_aka_runtime_failed"))?
            .map_err(live_stage_error)?;
            if let Some(auts) = aka_result.auts {
                return Ok(LiveRegisterAuthMaterial {
                    authorization: String::new(),
                    ims_esp_secrets: placeholder_ims_esp_secrets(),
                    ims_esp_alt_secrets: Vec::new(),
                    auts: Some(auts),
                });
            }
            if aka_result.res.is_empty() {
                return Err(live_stage_error("ims_aka_empty_response"));
            }
            let response = compute_aka_md5_response(
                &context.identity.private_user,
                &challenge.realm,
                &aka_result,
                &challenge.algorithm,
                "REGISTER",
                &digest_uri,
                &challenge.nonce,
                challenge.qop,
                &cnonce,
            )?;
            let selected_offer = select_live_security_server_offer(profile, challenge)?
                .ok_or_else(|| live_stage_error("ims_security_server_offer_missing"))?;
            let secrets = derive_ims_esp_secrets(&selected_offer, &aka_result)?;
            let alt_secrets = derive_ims_esp_secrets_raw_ik(&selected_offer, &aka_result)
                .map(|secrets| vec![secrets])
                .unwrap_or_default();
            (response, secrets, alt_secrets)
        }
        LiveDigestNonceKind::PlainDigest => {
            let response = compute_plain_md5_response(
                &context.identity.private_user,
                &challenge.realm,
                "REGISTER",
                &digest_uri,
                &challenge.nonce,
                challenge.qop,
                &cnonce,
            )?;
            (response, placeholder_ims_esp_secrets(), Vec::new())
        }
    };
    let authorization =
        build_digest_authorization_header(context, challenge, &digest_uri, &response, &cnonce)?;
    info!(
        auth_header = challenge.authorization_header_name(),
        security_verify_present = !challenge.security_server_values.is_empty(),
        nonce_kind = challenge.nonce_kind.label(),
        "IMS REGISTER authenticated request ready"
    );
    Ok(LiveRegisterAuthMaterial {
        authorization,
        ims_esp_secrets,
        ims_esp_alt_secrets,
        auts: None,
    })
}

fn parse_live_digest_challenge(
    response: &str,
    expected_realm: &str,
) -> Result<LiveDigestChallenge, LiveStageError> {
    let mut candidates = Vec::new();
    for value in sip_header_values(response, "www-authenticate") {
        candidates.extend(
            split_digest_challenge_values(&value)
                .into_iter()
                .map(|value| ("www_authenticate", value)),
        );
    }
    for value in sip_header_values(response, "proxy-authenticate") {
        candidates.extend(
            split_digest_challenge_values(&value)
                .into_iter()
                .map(|value| ("proxy_authenticate", value)),
        );
    }
    if candidates.is_empty() {
        return Err(live_stage_error("ims_digest_challenge_missing"));
    }

    let mut last_error = None;
    let mut accepted = Vec::new();
    for (header_kind, value) in candidates {
        log_live_digest_challenge_candidate(expected_realm, header_kind, &value);
        match parse_live_digest_challenge_value(response, expected_realm, header_kind, &value) {
            Ok(challenge) => accepted.push(challenge),
            Err(err) => last_error = Some(err),
        }
    }
    if let Some(challenge) = accepted
        .iter()
        .find(|challenge| challenge.algorithm.to_ascii_uppercase().starts_with("AKAV"))
        .cloned()
    {
        return Ok(challenge);
    }
    if let Some(challenge) = accepted.into_iter().next() {
        return Ok(challenge);
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_digest_challenge_missing")))
}

fn reject_plain_digest_when_disabled(
    profile: &'static CarrierProfile,
    challenge: &LiveDigestChallenge,
) -> Result<(), LiveStageError> {
    if challenge.nonce_kind == LiveDigestNonceKind::PlainDigest
        && !profile.ims.register.use_plain_digest_placeholder
    {
        warn!(
            profile_id = profile.meta.profile_id,
            algorithm = challenge.algorithm.as_str(),
            "IMS REGISTER plain MD5 digest challenge rejected by carrier policy"
        );
        return Err(live_stage_error("ims_digest_plain_md5_disabled"));
    }
    Ok(())
}

fn log_live_digest_challenge_candidate(
    expected_realm: &str,
    header_kind: &'static str,
    value: &str,
) {
    let params = parse_live_digest_params(value);
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let algorithm = param("algorithm").unwrap_or("AKAv1-MD5");
    let realm = param("realm").unwrap_or("");
    let nonce = param("nonce").unwrap_or("");
    let nonce_shape = digest_nonce_shape(nonce);
    let decoded_len = decode_digest_nonce(nonce).map(|bytes| bytes.len()).ok();
    info!(
        header_kind = header_kind,
        algorithm = algorithm,
        realm_profile_match = realm == expected_realm,
        realm_plmn_matches_profile = realm_plmn_matches_expected(realm, expected_realm),
        nonce_present = !nonce.is_empty(),
        nonce_text_len = nonce.len(),
        nonce_decoded_len = decoded_len.unwrap_or(0),
        nonce_is_aka_sized = decoded_len.is_some_and(|len| len >= 32),
        nonce_ascii_hex = nonce_shape.ascii_hex,
        nonce_base64_like = nonce_shape.base64_like,
        qop_present = param("qop").is_some(),
        opaque_present = param("opaque").is_some(),
        "IMS REGISTER digest challenge candidate metadata"
    );
}

fn parse_live_digest_challenge_value(
    response: &str,
    expected_realm: &str,
    header_kind: &'static str,
    value: &str,
) -> Result<LiveDigestChallenge, LiveStageError> {
    let params = parse_live_digest_params(value);
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let algorithm = param("algorithm").unwrap_or("AKAv1-MD5").to_string();
    if !algorithm.eq_ignore_ascii_case("AKAv1-MD5")
        && !algorithm.eq_ignore_ascii_case("AKAv2-MD5")
        && !algorithm.eq_ignore_ascii_case("MD5")
    {
        warn!(
            algorithm = algorithm.as_str(),
            "IMS REGISTER digest algorithm unsupported"
        );
        return Err(live_stage_error("ims_digest_algorithm_unsupported"));
    }
    let realm = param("realm")
        .ok_or_else(|| live_stage_error("ims_digest_realm_missing"))?
        .to_string();
    let realm_profile_match = realm == expected_realm;
    let realm_plmn = parse_realm_plmn(&realm);
    let realm_mcc = realm_plmn
        .as_ref()
        .map(|(mcc, _)| mcc.as_str())
        .unwrap_or("absent");
    let realm_mnc = realm_plmn
        .as_ref()
        .map(|(_, mnc)| mnc.as_str())
        .unwrap_or("absent");
    let realm_plmn_matches_profile = realm_plmn_matches_expected(&realm, expected_realm);
    info!(
        header_kind = header_kind,
        algorithm = algorithm.as_str(),
        realm_profile_match = realm_profile_match,
        realm_len = realm.len(),
        realm_is_3gpp = realm.ends_with(".3gppnetwork.org"),
        realm_contains_expected_domain = realm.contains(expected_realm),
        realm_mcc = realm_mcc,
        realm_mnc = realm_mnc,
        realm_plmn_matches_profile = realm_plmn_matches_profile,
        "IMS REGISTER digest challenge realm metadata received"
    );
    if realm != expected_realm {
        warn!("IMS REGISTER digest realm differs from profile realm");
    }
    let nonce = param("nonce")
        .ok_or_else(|| live_stage_error("ims_digest_nonce_missing"))?
        .to_string();
    let nonce_shape = digest_nonce_shape(&nonce);
    let nonce_bytes = decode_digest_nonce(&nonce)?;
    let nonce_kind = if nonce_bytes.len() >= 32 {
        LiveDigestNonceKind::AkaChallenge
    } else if algorithm.eq_ignore_ascii_case("MD5") && realm_plmn_matches_profile {
        warn!(
            algorithm = algorithm.as_str(),
            nonce_text_len = nonce.len(),
            nonce_len = nonce_bytes.len(),
            nonce_hex_candidate_len = nonce_shape.hex_decoded_len.unwrap_or(0),
            nonce_ascii_hex = nonce_shape.ascii_hex,
            nonce_base64_like = nonce_shape.base64_like,
            "IMS REGISTER digest nonce is plain MD5 challenge for home realm"
        );
        LiveDigestNonceKind::PlainDigest
    } else {
        warn!(
            algorithm = algorithm.as_str(),
            nonce_text_len = nonce.len(),
            nonce_len = nonce_bytes.len(),
            nonce_hex_candidate_len = nonce_shape.hex_decoded_len.unwrap_or(0),
            nonce_ascii_hex = nonce_shape.ascii_hex,
            nonce_base64_like = nonce_shape.base64_like,
            "IMS REGISTER digest nonce is not an AKA challenge"
        );
        return Err(live_stage_error("ims_digest_nonce_too_short"));
    };
    let qop = match param("qop") {
        Some(value)
            if value
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case("auth")) =>
        {
            Some("auth")
        }
        Some(_) => return Err(live_stage_error("ims_digest_qop_unsupported")),
        None => None,
    };

    let security_server_values = sip_header_values(response, "security-server");
    let security_server_offers = parse_live_security_server_offers(&security_server_values);
    info!(
        security_server_offer_count = security_server_offers.len(),
        "IMS REGISTER Security-Server offer metadata parsed"
    );

    Ok(LiveDigestChallenge {
        header_kind,
        realm,
        nonce,
        algorithm,
        qop,
        opaque: param("opaque").map(ToOwned::to_owned),
        rand: match nonce_kind {
            LiveDigestNonceKind::AkaChallenge => nonce_bytes[..16].to_vec(),
            LiveDigestNonceKind::PlainDigest => Vec::new(),
        },
        autn: match nonce_kind {
            LiveDigestNonceKind::AkaChallenge => nonce_bytes[16..32].to_vec(),
            LiveDigestNonceKind::PlainDigest => Vec::new(),
        },
        nonce_kind,
        security_server_values,
        security_server_offers,
    })
}

fn parse_live_security_server_offers(values: &[String]) -> Vec<LiveSecurityServerOffer> {
    values
        .iter()
        .flat_map(|value| split_sip_header_values(value))
        .filter_map(|value| parse_live_security_server_offer(&value).ok())
        .collect()
}

fn parse_live_security_server_offer(
    value: &str,
) -> Result<LiveSecurityServerOffer, LiveStageError> {
    let parts = value.split(';').map(str::trim).collect::<Vec<_>>();
    let mechanism = parts
        .first()
        .copied()
        .ok_or_else(|| live_stage_error("ims_security_server_offer_invalid"))?;
    if !mechanism.eq_ignore_ascii_case("ipsec-3gpp") {
        return Err(live_stage_error(
            "ims_security_server_mechanism_unsupported",
        ));
    }
    let params = parts
        .iter()
        .skip(1)
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((
                key.trim().to_ascii_lowercase(),
                trim_digest_value(value).to_string(),
            ))
        })
        .collect::<Vec<_>>();
    let param = |name: &str| {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let alg = param("alg").unwrap_or("hmac-sha-1-96").to_ascii_lowercase();
    let ealg = param("ealg").unwrap_or("aes-cbc").to_ascii_lowercase();
    let protocol = param("prot").unwrap_or("esp").to_ascii_lowercase();
    let mode = param("mod").unwrap_or("trans").to_ascii_lowercase();
    let spi_c = parse_u32_param(param("spi-c"))
        .ok_or_else(|| live_stage_error("ims_security_server_spi_missing"))?;
    let spi_s = parse_u32_param(param("spi-s"))
        .ok_or_else(|| live_stage_error("ims_security_server_spi_missing"))?;
    let runtime_config = live_runtime_config();
    let port_c = parse_u16_param(param("port-c")).unwrap_or(runtime_config.ims_security_port_c);
    let port_s = parse_u16_param(param("port-s")).unwrap_or(runtime_config.ims_security_port_s);
    let q_milli = parse_q_milli(param("q")).unwrap_or(1000);

    Ok(LiveSecurityServerOffer {
        raw: value.trim().to_string(),
        alg,
        ealg,
        protocol,
        mode,
        spi_c,
        spi_s,
        port_c,
        port_s,
        q_milli,
    })
}

fn select_live_security_server_offer(
    profile: &'static CarrierProfile,
    challenge: &LiveDigestChallenge,
) -> Result<Option<LiveSecurityServerOffer>, LiveStageError> {
    if challenge.security_server_offers.is_empty() {
        return Ok(None);
    }
    let mut offers = challenge.security_server_offers.clone();
    offers.sort_by_key(|offer| std::cmp::Reverse(offer.q_milli));
    for offer in offers {
        if live_security_offer_matches_profile(profile, &offer) {
            return Ok(Some(offer));
        }
    }
    if profile.ims.register.strict_security_server_offer {
        Err(live_stage_error("ims_security_server_offer_unmatched"))
    } else {
        Ok(challenge.security_server_offers.first().cloned())
    }
}

fn live_security_offer_matches_profile(
    profile: &'static CarrierProfile,
    offer: &LiveSecurityServerOffer,
) -> bool {
    profile
        .ims
        .register
        .security_client_mechanisms
        .iter()
        .any(|mechanism| {
            let mut parts = mechanism.split('/');
            let alg = parts.next().unwrap_or("hmac-sha-1-96");
            let ealg = parts.next().unwrap_or("aes-cbc");
            let protocol = parts.next().unwrap_or("esp");
            let mode = parts.next().unwrap_or("trans");
            alg.eq_ignore_ascii_case(&offer.alg)
                && ealg.eq_ignore_ascii_case(&offer.ealg)
                && protocol.eq_ignore_ascii_case(&offer.protocol)
                && mode.eq_ignore_ascii_case(&offer.mode)
        })
}

fn parse_u32_param(value: Option<&str>) -> Option<u32> {
    value.and_then(|value| value.parse::<u32>().ok())
}

fn parse_u16_param(value: Option<&str>) -> Option<u16> {
    value.and_then(|value| value.parse::<u16>().ok())
}

fn parse_q_milli(value: Option<&str>) -> Option<u16> {
    let value = value?;
    let (whole, frac) = value.split_once('.').unwrap_or((value, ""));
    let whole = whole.parse::<u16>().ok()?;
    let frac = frac
        .chars()
        .take(3)
        .chain(std::iter::repeat('0'))
        .take(3)
        .collect::<String>()
        .parse::<u16>()
        .ok()?;
    whole
        .checked_mul(1000)
        .and_then(|base| base.checked_add(frac))
}

impl LiveDigestChallenge {
    fn authorization_header_name(&self) -> &'static str {
        match self.header_kind {
            "proxy_authenticate" => "Proxy-Authorization",
            _ => "Authorization",
        }
    }
}

fn sip_header_values(response: &str, header_name: &str) -> Vec<String> {
    response
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(header_name)
                .then(|| value.trim().to_string())
        })
        .collect()
}

fn split_sip_header_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    let mut in_quote = false;

    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ',' if !in_quote => {
                let item = current.trim();
                if !item.is_empty() {
                    values.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let item = current.trim();
    if !item.is_empty() {
        values.push(item.to_string());
    }
    values
}

fn split_digest_challenge_values(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }

    let mut values = Vec::new();
    let mut start = 0usize;
    let mut escaped = false;
    let mut in_quote = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                if let Some(next_start) = digest_scheme_start_after_comma(value, index) {
                    let item = value[start..index].trim();
                    if !item.is_empty() {
                        values.push(item.to_string());
                    }
                    start = next_start;
                }
            }
            _ => {}
        }
    }

    let item = value[start..].trim();
    if !item.is_empty() {
        values.push(item.to_string());
    }
    values
}

fn digest_scheme_start_after_comma(value: &str, comma_index: usize) -> Option<usize> {
    let rest = value.get(comma_index + 1..)?;
    let trimmed = rest.trim_start();
    let skipped = rest.len() - trimmed.len();
    starts_with_digest_scheme(trimmed).then_some(comma_index + 1 + skipped)
}

fn starts_with_digest_scheme(value: &str) -> bool {
    let Some(prefix) = value.get(..6) else {
        return false;
    };
    prefix.eq_ignore_ascii_case("Digest")
        && value
            .get(6..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(char::is_whitespace)
}

fn parse_live_digest_params(value: &str) -> Vec<(String, String)> {
    let value = value
        .trim()
        .strip_prefix("Digest")
        .map(str::trim)
        .unwrap_or_else(|| value.trim());
    split_digest_param_list(value)
        .into_iter()
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.trim().to_string(), trim_digest_value(value).to_string()))
        })
        .collect()
}

fn split_digest_param_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    let mut in_quote = false;
    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                in_quote = !in_quote;
                current.push(ch);
            }
            ',' if !in_quote => {
                let item = current.trim();
                if !item.is_empty() {
                    items.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let item = current.trim();
    if !item.is_empty() {
        items.push(item.to_string());
    }
    items
}

fn trim_digest_value(value: &str) -> &str {
    value.trim().trim_matches('"')
}

fn parse_realm_plmn(value: &str) -> Option<(String, String)> {
    let mut mcc = None;
    let mut mnc = None;
    for part in value.split('.') {
        if let Some(rest) = part.strip_prefix("mcc") {
            if rest.len() == 3 && rest.chars().all(|ch| ch.is_ascii_digit()) {
                mcc = Some(rest.to_string());
            }
        }
        if let Some(rest) = part.strip_prefix("mnc") {
            if (rest.len() == 2 || rest.len() == 3) && rest.chars().all(|ch| ch.is_ascii_digit()) {
                mnc = Some(rest.to_string());
            }
        }
    }
    Some((mcc?, mnc?))
}

fn realm_plmn_matches_expected(realm: &str, expected_realm: &str) -> bool {
    let Some((realm_mcc, realm_mnc)) = parse_realm_plmn(realm) else {
        return false;
    };
    let Some((expected_mcc, expected_mnc)) = parse_realm_plmn(expected_realm) else {
        return false;
    };
    realm_mcc == expected_mcc
        && realm_mnc.trim_start_matches('0') == expected_mnc.trim_start_matches('0')
}

fn decode_digest_nonce(value: &str) -> Result<Vec<u8>, LiveStageError> {
    let trimmed = value.trim();
    if trimmed.len().is_multiple_of(2)
        && !trimmed.is_empty()
        && trimmed.bytes().all(|b| b.is_ascii_hexdigit())
    {
        let mut out = Vec::with_capacity(trimmed.len() / 2);
        for chunk in trimmed.as_bytes().chunks_exact(2) {
            let high = hex_digit_value(chunk[0])?;
            let low = hex_digit_value(chunk[1])?;
            out.push((high << 4) | low);
        }
        return Ok(out);
    }

    BASE64_STANDARD
        .decode(value.as_bytes())
        .or_else(|_| {
            let mut padded = value.to_string();
            while !padded.len().is_multiple_of(4) {
                padded.push('=');
            }
            BASE64_STANDARD.decode(padded.as_bytes())
        })
        .map_err(|_| live_stage_error("ims_digest_nonce_decode_failed"))
}

fn hex_digit_value(byte: u8) -> Result<u8, LiveStageError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(live_stage_error("ims_digest_nonce_decode_failed")),
    }
}

#[derive(Debug, Clone, Copy)]
struct DigestNonceShape {
    ascii_hex: bool,
    base64_like: bool,
    hex_decoded_len: Option<usize>,
}

fn digest_nonce_shape(value: &str) -> DigestNonceShape {
    let trimmed = value.trim();
    let ascii_hex = trimmed.len().is_multiple_of(2)
        && !trimmed.is_empty()
        && trimmed.bytes().all(|b| b.is_ascii_hexdigit());
    let base64_like = !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'));
    DigestNonceShape {
        ascii_hex,
        base64_like,
        hex_decoded_len: ascii_hex.then_some(trimmed.len() / 2),
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_aka_md5_response(
    username: &str,
    realm: &str,
    aka: &super::qmi_uim::UsimAkaApduResult,
    algorithm: &str,
    method: &str,
    digest_uri: &str,
    nonce: &str,
    qop: Option<&str>,
    cnonce: &str,
) -> Result<String, LiveStageError> {
    let password = aka_digest_password(algorithm, aka)?;
    let mut a1 = Vec::with_capacity(username.len() + realm.len() + password.len() + 2);
    a1.extend_from_slice(username.as_bytes());
    a1.push(b':');
    a1.extend_from_slice(realm.as_bytes());
    a1.push(b':');
    a1.extend_from_slice(&password);
    let ha1 = md5_hex(&a1);
    let ha2 = md5_hex(format!("{method}:{digest_uri}").as_bytes());
    let proof_input = match qop {
        Some("auth") => format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}"),
        Some(_) => return Err(live_stage_error("ims_digest_qop_unsupported")),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    Ok(md5_hex(proof_input.as_bytes()))
}

fn compute_plain_md5_response(
    username: &str,
    realm: &str,
    method: &str,
    digest_uri: &str,
    nonce: &str,
    qop: Option<&str>,
    cnonce: &str,
) -> Result<String, LiveStageError> {
    let ha1 = md5_hex(format!("{username}:{realm}:").as_bytes());
    let ha2 = md5_hex(format!("{method}:{digest_uri}").as_bytes());
    let proof_input = match qop {
        Some("auth") => format!("{ha1}:{nonce}:00000001:{cnonce}:auth:{ha2}"),
        Some(_) => return Err(live_stage_error("ims_digest_qop_unsupported")),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    Ok(md5_hex(proof_input.as_bytes()))
}

fn aka_digest_password(
    algorithm: &str,
    aka: &super::qmi_uim::UsimAkaApduResult,
) -> Result<Vec<u8>, LiveStageError> {
    // Delegate the RFC 3310/4169 password derivation to the shared IMS core,
    // remapping its neutral reason codes onto the live-specific ones so all
    // existing call sites and tests keep their `ims_*` error semantics.
    let material = crate::connectivity::core::digest_aka::AkaMaterial {
        res: &aka.res,
        ck: &aka.ck,
        ik: &aka.ik,
    };
    crate::connectivity::core::digest_aka::aka_digest_password(algorithm, &material).map_err(|err| {
        let code = match err.code() {
            "aka_res_empty" => "ims_aka_empty_response",
            "aka_material_invalid" => "ims_aka_material_invalid",
            _ => "ims_digest_algorithm_unsupported",
        };
        live_stage_error(code)
    })
}

fn derive_ims_esp_secrets(
    offer: &LiveSecurityServerOffer,
    aka: &super::qmi_uim::UsimAkaApduResult,
) -> Result<ChildSaSecretPair, LiveStageError> {
    derive_ims_esp_secrets_with_integrity_key(offer, aka, true)
}

fn derive_ims_esp_secrets_raw_ik(
    offer: &LiveSecurityServerOffer,
    aka: &super::qmi_uim::UsimAkaApduResult,
) -> Result<ChildSaSecretPair, LiveStageError> {
    derive_ims_esp_secrets_with_integrity_key(offer, aka, false)
}

fn derive_ims_esp_secrets_with_integrity_key(
    offer: &LiveSecurityServerOffer,
    aka: &super::qmi_uim::UsimAkaApduResult,
    expand_ik_to_hmac_sha1_key: bool,
) -> Result<ChildSaSecretPair, LiveStageError> {
    if !offer.alg.eq_ignore_ascii_case("hmac-sha-1-96")
        || !offer.ealg.eq_ignore_ascii_case("aes-cbc")
        || !offer.protocol.eq_ignore_ascii_case("esp")
        || !offer.mode.eq_ignore_ascii_case("trans")
    {
        return Err(live_stage_error("ims_security_server_offer_unsupported"));
    }
    if aka.ck.len() < 16 || aka.ik.len() < 16 {
        return Err(live_stage_error("ims_aka_material_invalid"));
    }
    let encryption_key = aka.ck[..16].to_vec();
    let integrity_key = if expand_ik_to_hmac_sha1_key {
        ims_hmac_sha1_96_key(&aka.ik[..16])
    } else {
        aka.ik[..16].to_vec()
    };
    let plan = ChildSaKeySchedulePlan {
        encryption: "aes_cbc",
        integrity: "hmac_sha1_96",
        encryption_key_bytes: 16,
        integrity_key_bytes: integrity_key.len(),
        direction_secret_bytes: 16 + integrity_key.len(),
        total_secret_bytes: (16 + integrity_key.len()) * 2,
        exported_secret_values: false,
        sensitive_values_policy: "ims_ipsec3gpp_secret_bytes_redacted_and_zeroed_on_drop",
    };
    Ok(ChildSaSecretPair::from_protocol_parts(
        plan,
        encryption_key.clone(),
        integrity_key.clone(),
        encryption_key,
        integrity_key,
    ))
}

fn ims_hmac_sha1_96_key(ik: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(20);
    key.extend_from_slice(ik);
    key.resize(20, 0);
    key
}

fn placeholder_ims_esp_secrets() -> ChildSaSecretPair {
    let plan = ChildSaKeySchedulePlan {
        encryption: "aes_cbc",
        integrity: "hmac_sha1_96",
        encryption_key_bytes: 16,
        integrity_key_bytes: 20,
        direction_secret_bytes: 36,
        total_secret_bytes: 72,
        exported_secret_values: false,
        sensitive_values_policy: "placeholder_not_used_without_security_server",
    };
    ChildSaSecretPair::from_protocol_parts(plan, vec![0; 16], vec![0; 20], vec![0; 16], vec![0; 20])
}

fn hmac_md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    // Single shared implementation lives in the IMS core (RFC 2104).
    crate::connectivity::core::digest_aka::hmac_md5(key, data)
}

fn build_digest_authorization_header(
    context: &LiveRegisterRequestContext,
    challenge: &LiveDigestChallenge,
    digest_uri: &str,
    response: &str,
    cnonce: &str,
) -> Result<String, LiveStageError> {
    let mut header = format!(
        "{}: Digest username=\"{}\",realm=\"{}\",nonce=\"{}\",uri=\"{}\",response=\"{}\",algorithm={}",
        challenge.authorization_header_name(),
        quote_sip_param(&context.identity.private_user),
        quote_sip_param(&challenge.realm),
        quote_sip_param(&challenge.nonce),
        quote_sip_param(digest_uri),
        response,
        challenge.algorithm
    );
    if let Some(qop) = challenge.qop {
        header.push_str(&format!(",qop={qop},nc=00000001,cnonce=\"{cnonce}\""));
    }
    if let Some(opaque) = challenge.opaque.as_deref() {
        header.push_str(&format!(",opaque=\"{}\"", quote_sip_param(opaque)));
    }
    Ok(header)
}

fn build_digest_resync_authorization_header(
    context: &LiveRegisterRequestContext,
    challenge: &LiveDigestChallenge,
    digest_uri: &str,
    auts: &[u8],
) -> Result<String, LiveStageError> {
    if auts.is_empty() {
        return Err(live_stage_error("ims_aka_auts_empty"));
    }
    let auts = BASE64_STANDARD.encode(auts);
    let mut header = format!(
        "{}: Digest username=\"{}\",realm=\"{}\",nonce=\"{}\",uri=\"{}\",response=\"\",algorithm={},auts=\"{}\"",
        challenge.authorization_header_name(),
        quote_sip_param(&context.identity.private_user),
        quote_sip_param(&challenge.realm),
        quote_sip_param(&challenge.nonce),
        quote_sip_param(digest_uri),
        challenge.algorithm,
        quote_sip_param(&auts)
    );
    if let Some(qop) = challenge.qop {
        header.push_str(&format!(
            ",qop={qop},nc=00000001,cnonce=\"{}\"",
            live_digest_cnonce()?
        ));
    }
    if let Some(opaque) = challenge.opaque.as_deref() {
        header.push_str(&format!(",opaque=\"{}\"", quote_sip_param(opaque)));
    }
    Ok(header)
}

fn live_digest_cnonce() -> Result<String, LiveStageError> {
    Ok(random_bytes(8)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

fn quote_sip_param(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn format_sip_instance_id(_profile: &'static CarrierProfile) -> Result<String, LiveStageError> {
    let mut bytes = random_bytes(16)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "urn:uuid:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn build_live_user_agent(profile: &'static CarrierProfile, format: LiveUserAgentFormat) -> String {
    match format {
        LiveUserAgentFormat::ProfileDefault => profile.ims.user_agent.to_string(),
        LiveUserAgentFormat::DeviceModelFocused => {
            let model = profile.identity.device_model_hint.trim();
            if model.is_empty() {
                "SimAdmin VoWiFi".to_string()
            } else {
                format!("{model} VoWiFi")
            }
        }
    }
}

fn build_security_client_header(
    profile: &'static CarrierProfile,
    format: LiveSecurityClientFormat,
    state: &LiveSecurityClientState,
) -> String {
    let mechanism = profile
        .ims
        .register
        .security_client_mechanisms
        .first()
        .copied()
        .unwrap_or("hmac-sha-1-96/aes-cbc/esp/trans");
    let mut parts = mechanism.split('/');
    let alg = parts.next().unwrap_or("hmac-sha-1-96");
    let ealg = parts.next().unwrap_or("aes-cbc");
    let protocol = parts.next().unwrap_or("esp");
    let mode = parts.next().unwrap_or("trans");
    match format {
        LiveSecurityClientFormat::FullSpaced => format!(
            "ipsec-3gpp; alg={alg}; ealg={ealg}; prot={protocol}; mod={mode}; spi-c={}; spi-s={}; port-c={}; port-s={}",
            state.spi_c,
            state.spi_s,
            state.port_c, state.port_s
        ),
        LiveSecurityClientFormat::FullCompact => format!(
            "ipsec-3gpp;alg={alg};ealg={ealg};prot={protocol};mod={mode};spi-c={};spi-s={};port-c={};port-s={}",
            state.spi_c,
            state.spi_s,
            state.port_c, state.port_s
        ),
        LiveSecurityClientFormat::MinimalSpaced => format!(
            "ipsec-3gpp; alg={alg}; ealg={ealg}; spi-c={}; spi-s={}; port-c={}; port-s={}",
            state.spi_c,
            state.spi_s,
            state.port_c, state.port_s
        ),
    }
}

fn sip_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(addr) => addr.to_string(),
        IpAddr::V6(addr) => format!("[{addr}]"),
    }
}

/// Decide whether this REGISTER carries sec-agree headers.
///
/// `sec_agree_mode` is the carrier-configurable answer and wins:
/// - `required` — always send them.
/// - `disabled` — never send them, even if a retry variant asks for it. Some
///   carriers reject a REGISTER that offers sec-agree they did not ask for.
/// - `auto` — fall back to the legacy boolean plus whatever the retry variant
///   is currently probing.
///
/// A mismatch here makes REGISTER fail outright, which is why it is settable.
fn sec_agree_headers_required(profile: &'static CarrierProfile, force_from_variant: bool) -> bool {
    match profile.ims.register.sec_agree_mode {
        "required" => true,
        "disabled" => false,
        _ => profile.ims.register.require_sec_agree_headers || force_from_variant,
    }
}

/// Build `P-Access-Network-Info`. The access type comes from the profile
/// because carriers that validate this header reject a wrong one, and the
/// correct value differs per carrier (`IEEE-802.11`, `IEEE-802.11a`, …).
fn build_p_access_network_info(profile: &'static CarrierProfile) -> String {
    let access_type = profile.ims.register.access_network_info.trim();
    let access_type = if access_type.is_empty() {
        profiles::DEFAULT_ACCESS_NETWORK_INFO
    } else {
        access_type
    };
    if profile.ims.register.include_pani_authenticated {
        format!("{access_type};i-wlan-node-id=000000000000;network-provided")
    } else {
        format!("{access_type};i-wlan-node-id=000000000000")
    }
}

fn build_cellular_network_info(profile: &'static CarrierProfile) -> String {
    format!(
        "3GPP-E-UTRAN-FDD;utran-cell-id-3gpp={}0000000;cell-info-age=0",
        profile.meta.plmn
    )
}

fn three_digit_mnc(profile: &'static CarrierProfile) -> String {
    format!("{:0>3}", profile.meta.mnc)
}

fn random_u32_nonzero() -> Result<u32, LiveStageError> {
    let bytes = random_bytes(4)?;
    let value = u32::from_be_bytes(bytes.try_into().expect("fixed length"));
    if value == 0 {
        random_u32_nonzero()
    } else {
        Ok(value)
    }
}

fn hex_token(bytes: usize) -> String {
    random_bytes(bytes)
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .unwrap_or_else(|_| "simadmin".to_string())
}

fn sip_instance_uuid() -> String {
    let bytes = random_bytes(16).unwrap_or_else(|_| vec![0; 16]);
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn ip_family_name(addr: IpAddr) -> &'static str {
    match addr {
        IpAddr::V4(_) => "ipv4",
        IpAddr::V6(_) => "ipv6",
    }
}

async fn recv_ike_response_with_retransmit(
    transport: &UdpSocketDatagramTransport,
    destination: SocketAddr,
    request: &[u8],
    use_nat_t: bool,
    timeout_reason: &'static str,
    attempts: usize,
) -> Result<Vec<u8>, LiveStageError> {
    let mut last_error = None;
    for attempt in 0..attempts {
        debug!(
            "recv_ike_response_with_retransmit: waiting for packet, attempt {}/{}",
            attempt + 1,
            attempts
        );
        match transport.recv_ike_message_metadata(use_nat_t).await {
            Ok((remote, response, _metadata)) => {
                debug!(
                    "Received IKE packet from remote={:?}, len={}",
                    remote,
                    response.len()
                );
                return Ok(response);
            }
            Err(TransportError::Timeout(_)) if attempt + 1 < attempts => {
                warn!("Timeout receiving response from remote={:?}. Retransmitting request (attempt {}/{})", destination, attempt + 1, attempts);
                last_error = Some(live_stage_error(timeout_reason));
                transport
                    .send_ike_message_metadata(use_nat_t, destination, request)
                    .await
                    .map_err(map_transport_error)?;
            }
            Err(TransportError::Timeout(err)) => {
                error!(
                    "Timeout receiving response from remote={:?}: {}. No more attempts.",
                    destination, err
                );
                return Err(live_stage_error(timeout_reason));
            }
            Err(error) => {
                error!(
                    "Transport error receiving response from remote={:?}: {:?}",
                    destination, error
                );
                return Err(map_transport_error(error));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error(timeout_reason)))
}

async fn live_ike_identity(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Result<String, LiveStageError> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("ike_identity_unavailable"))?;
    // The IKE identity must be built from THIS line's IMSI, otherwise a second
    // line would present the first line's subscriber to the ePDG.
    let sim = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("ike_identity_unavailable"))?;
    build_permanent_nai(profile, &sim.imsi).map_err(map_identity_error)
}

fn map_identity_error(error: IkeIdentityError) -> LiveStageError {
    live_stage_error(match error {
        IkeIdentityError::EmptyImsi | IkeIdentityError::InvalidImsi => "ike_identity_unavailable",
        IkeIdentityError::ImsiPlmnMismatch => "ike_identity_profile_mismatch",
    })
}

fn validate_ike_auth_response(
    response: &[u8],
    initiator_spi: u64,
    message_id: u32,
) -> Result<(), LiveStageError> {
    let matches = encrypted_response_header_matches(
        response,
        initiator_spi,
        IkeExchangeType::IkeAuth,
        message_id,
    )
    .map_err(|_| live_stage_error("ike_auth_response_decode_failed"))?;
    if !matches {
        return Err(live_stage_error("ike_auth_response_header_mismatch"));
    }
    Ok(())
}

fn generate_initiator_spi() -> Result<u64, LiveStageError> {
    let bytes = random_bytes(8)?;
    let spi = u64::from_be_bytes(bytes.try_into().expect("fixed length"));
    if spi == 0 {
        return generate_initiator_spi();
    }
    Ok(spi)
}

fn generate_nonce() -> Result<Vec<u8>, LiveStageError> {
    random_bytes(LIVE_IKE_NONCE_BYTES)
}

fn random_bytes(len: usize) -> Result<Vec<u8>, LiveStageError> {
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = vec![0u8; len];
    ring::rand::SecureRandom::fill(&rng, &mut bytes)
        .map_err(|_| live_stage_error("runtime_random_unavailable"))?;
    Ok(bytes)
}

fn unspecified_local_addr_for(remote: SocketAddr) -> SocketAddr {
    match remote.ip() {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

async fn local_bind_addr_for_destination(
    remote: SocketAddr,
    preferred_port: u16,
) -> Result<SocketAddr, TransportError> {
    let probe = tokio::net::UdpSocket::bind(unspecified_local_addr_for(remote))
        .await
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;
    probe
        .connect(remote)
        .await
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;
    let local = probe
        .local_addr()
        .map_err(|err| TransportError::Io(err.kind().to_string()))?;
    let preferred = SocketAddr::new(local.ip(), preferred_port);
    match tokio::net::UdpSocket::bind(preferred).await {
        Ok(socket) => Ok(socket
            .local_addr()
            .map_err(|err| TransportError::Io(err.kind().to_string()))?),
        Err(_) => Ok(SocketAddr::new(local.ip(), 0)),
    }
}

fn map_transport_error(error: TransportError) -> LiveStageError {
    let reason = match error {
        TransportError::DnsFailed(_) => "epdg_dns_resolution_failed",
        TransportError::RouteUnavailable(_) => "network_route_unavailable",
        TransportError::UnsupportedProxy(_) => "proxy_transport_unsupported",
        TransportError::Io(_) => "udp_transport_io_failed",
        TransportError::Timeout(_) => "udp_transport_timeout",
    };
    live_stage_error(reason)
}

fn map_dataplane_state_error(error: DataplaneStateError) -> LiveStageError {
    let reason = match error {
        DataplaneStateError::EmptyEspProposals => "profile_missing_esp_proposal",
        DataplaneStateError::InvalidSaIdentifier => "live_child_sa_identifier_invalid",
        DataplaneStateError::InvalidSelectedEspProposal => {
            "live_child_sa_esp_proposal_not_profile_allowed"
        }
        DataplaneStateError::EspPacketTooShort => "live_esp_packet_too_short",
        DataplaneStateError::InvalidPhase { .. } => "live_esp_dataplane_phase_invalid",
        DataplaneStateError::SequenceExhausted => "live_esp_sequence_exhausted",
        DataplaneStateError::InnerPacketTooLarge { .. } => "live_esp_inner_packet_too_large",
        DataplaneStateError::InnerQueueFull => "live_esp_inner_queue_full",
        DataplaneStateError::EspIntegrityMismatch => "live_esp_integrity_mismatch",
        DataplaneStateError::EspInvalidPadding => "live_esp_invalid_padding",
        DataplaneStateError::EspUnsupportedCipher => "live_esp_unsupported_cipher",
        DataplaneStateError::EspUnsupportedIntegrity => "live_esp_unsupported_integrity",
        DataplaneStateError::EspRandomFailed => "live_esp_random_failed",
    };
    live_stage_error(reason)
}

fn live_stage_error(reason: impl Into<String>) -> LiveStageError {
    LiveStageError {
        reason: reason.into(),
    }
}

#[derive(Debug, Clone)]
pub struct LiveNetworkStageAdapter<E, D> {
    epdg: E,
    datagram: D,
}

impl<E, D> LiveNetworkStageAdapter<E, D> {
    pub fn new(epdg: E, datagram: D) -> Self {
        Self { epdg, datagram }
    }
}

impl<E, D> LiveStageAdapter for LiveNetworkStageAdapter<E, D>
where
    E: LiveEpdgAdapter,
    D: LiveDatagramAdapter,
{
    fn run_stage<'a>(
        &'a self,
        stage: ExecutorStage,
        profile: &'static CarrierProfile,
    ) -> LiveAdapterFuture<'a> {
        Box::pin(async move {
            match stage {
                ExecutorStage::Epdg => {
                    let endpoint = self.epdg.resolve_epdg(profile).await?;
                    Ok(LiveStageObservation {
                        stage: stage.as_str(),
                        ready: !endpoint.addresses.is_empty(),
                        detail: "epdg_resolution_ready",
                        sensitive_values_policy: "endpoint_metadata_only_no_identity_values",
                    })
                }
                ExecutorStage::Ike
                | ExecutorStage::ChildSa
                | ExecutorStage::Esp
                | ExecutorStage::ImsRegister
                | ExecutorStage::Sms
                | ExecutorStage::Voice => {
                    self.datagram.check_udp_path(stage, profile).await?;
                    Ok(LiveStageObservation {
                        stage: stage.as_str(),
                        ready: true,
                        detail: "datagram_path_ready",
                        sensitive_values_policy: "path_state_only_no_packet_payload",
                    })
                }
                ExecutorStage::SimAuth => {
                    let conn = zbus::Connection::system()
                        .await
                        .map_err(|_| live_stage_error("sim_dbus_connection_failed"))?;
                    let identity = current_sim_identity(&conn)
                        .await
                        .ok_or_else(|| live_stage_error("sim_identity_not_ready"))?;
                    if identity.imsi.is_empty() {
                        return Err(live_stage_error("sim_imsi_empty"));
                    }
                    verify_live_sim_auth_access().await?;
                    info!("SimAuth stage verification: identity and UIM access are ready");
                    Ok(LiveStageObservation {
                        stage: stage.as_str(),
                        ready: true,
                        detail: "sim_auth_ready",
                        sensitive_values_policy: "metadata_only",
                    })
                }
                ExecutorStage::EsimRestore => {
                    info!("EsimRestore stage verification: restore state manager ready");
                    Ok(LiveStageObservation {
                        stage: stage.as_str(),
                        ready: true,
                        detail: "esim_restore_ready",
                        sensitive_values_policy: "metadata_only",
                    })
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BlockedLiveStageAdapter;

impl LiveStageAdapter for BlockedLiveStageAdapter {
    fn run_stage<'a>(
        &'a self,
        _stage: ExecutorStage,
        _profile: &'static CarrierProfile,
    ) -> LiveAdapterFuture<'a> {
        Box::pin(async { Err(live_stage_error("live_adapter_not_configured")) })
    }
}

pub struct LiveStageRunner<A> {
    gate: LiveExecutorGateReport,
    profile: &'static CarrierProfile,
    adapter: A,
}

impl<A> LiveStageRunner<A>
where
    A: LiveStageAdapter,
{
    pub fn new(gate: LiveExecutorGateReport, profile: &'static CarrierProfile, adapter: A) -> Self {
        Self {
            gate,
            profile,
            adapter,
        }
    }

    pub async fn run(&self, request: ExecutorStageRequest) -> ExecutorStageResult {
        if let Some(reason) = gate_blocker_for_stage(request.stage, &self.gate) {
            return stage_result(
                request.stage,
                ExecutorStageStatus::Skipped,
                Some(reason.to_string()),
            );
        }

        match self.adapter.run_stage(request.stage, self.profile).await {
            Ok(observation) if observation.ready => {
                stage_result(request.stage, ExecutorStageStatus::Completed, None)
            }
            Ok(observation) => stage_result(
                request.stage,
                ExecutorStageStatus::Failed,
                Some(observation.detail.to_string()),
            ),
            Err(err) => stage_result(request.stage, ExecutorStageStatus::Failed, Some(err.reason)),
        }
    }
}

pub fn gate_blocker_for_stage(
    stage: ExecutorStage,
    gate: &LiveExecutorGateReport,
) -> Option<&'static str> {
    if !live_stage_implemented(stage) {
        return Some("live_stage_not_implemented");
    }
    if stage_requires_live_network(stage) && !gate.effective_live_network_allowed {
        return Some("live_network_executor_disabled");
    }
    if stage_requires_device_change(stage) && !gate.effective_device_state_changes_allowed {
        return Some("device_state_change_executor_disabled");
    }
    None
}

pub fn live_stage_implemented(stage: ExecutorStage) -> bool {
    matches!(
        stage,
        ExecutorStage::EsimRestore
            | ExecutorStage::SimAuth
            | ExecutorStage::Epdg
            | ExecutorStage::Ike
            | ExecutorStage::ChildSa
            | ExecutorStage::Esp
            | ExecutorStage::ImsRegister
            | ExecutorStage::Sms
            | ExecutorStage::Voice
    )
}

pub fn live_transport_implemented(stage_id: &str) -> bool {
    matches!(stage_id, "udp_transport")
}

pub fn live_runtime_implementation_complete() -> bool {
    super::executor::EXECUTOR_STAGES
        .iter()
        .copied()
        .all(live_stage_implemented)
}

pub fn live_network_implementation_available() -> bool {
    super::executor::EXECUTOR_STAGES
        .iter()
        .copied()
        .any(|stage| stage_requires_live_network(stage) && live_stage_implemented(stage))
}

pub fn live_device_change_implementation_available() -> bool {
    super::executor::EXECUTOR_STAGES
        .iter()
        .copied()
        .any(|stage| stage_requires_device_change(stage) && live_stage_implemented(stage))
}

pub fn stage_requires_live_network(stage: ExecutorStage) -> bool {
    matches!(
        stage,
        ExecutorStage::Epdg
            | ExecutorStage::Ike
            | ExecutorStage::ChildSa
            | ExecutorStage::Esp
            | ExecutorStage::ImsRegister
            | ExecutorStage::Sms
            | ExecutorStage::Voice
    )
}

pub fn stage_requires_device_change(stage: ExecutorStage) -> bool {
    matches!(
        stage,
        ExecutorStage::EsimRestore | ExecutorStage::SimAuth | ExecutorStage::Sms
    )
}

fn stage_result(
    stage: ExecutorStage,
    status: ExecutorStageStatus,
    reason: Option<String>,
) -> ExecutorStageResult {
    ExecutorStageResult {
        stage: stage.as_str(),
        status: status.as_str(),
        readiness_key: readiness_key_for_stage(stage),
        reason,
        soak_observation: Some(soak_observation_for_stage(stage)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_network_overrides_apply_custom_dns_and_profile_pin() {
        let config = LineVowifiConfig {
            dns_server: "2001:4860:4860::8888".to_string(),
            profile_id: Some("gb_ee_23433".to_string()),
            ..LineVowifiConfig::default()
        };

        let overrides = build_live_network_overrides(&config).expect("valid network overrides");

        assert_eq!(overrides.profile_id.as_deref(), Some("gb_ee_23433"));
        assert_eq!(
            overrides.dns_server,
            Some("2001:4860:4860::8888".parse().unwrap())
        );
    }

    #[test]
    fn tun_names_are_unique_per_line_and_fit_ifnamsiz() {
        // Two connected lines must never be handed the same interface name: they
        // would collide in the kernel and the second tunnel would fail or hijack
        // the first one's device.
        let a = tun_name_for_line("sa_vwf0", "line-0123456789abcdef0123456789abcdef");
        let b = tun_name_for_line("sa_vwf0", "line-fedcba9876543210fedcba9876543210");
        assert_ne!(a, b, "distinct lines must get distinct devices");
        for name in [&a, &b] {
            assert!(
                name.len() <= MAX_IFNAME_LEN,
                "{name} exceeds IFNAMSIZ-1 ({} chars)",
                name.len()
            );
            assert!(
                name.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
                "{name} has characters the kernel will reject"
            );
        }

        // Stable across calls, so a reconnect reclaims its own device instead of
        // leaking a new interface each time.
        assert_eq!(
            a,
            tun_name_for_line("sa_vwf0", "line-0123456789abcdef0123456789abcdef")
        );

        // No line context keeps the configured base name, preserving the
        // single-line behaviour and the SIMADMIN_VOWIFI_TUN_NAME override.
        assert_eq!(tun_name_for_line("sa_vwf0", ""), "sa_vwf0");
        assert_eq!(tun_name_for_line("custom", ""), "custom");
    }

    #[test]
    fn line_network_overrides_reject_unimplemented_proxy_transport() {
        // UDP Relay has no client implementation: a private relay protocol adds
        // nothing over pointing the line at a self-hosted standard SOCKS5 server.
        let config = LineVowifiConfig {
            proxy_mode: VowifiProxyMode::UdpRelay,
            proxy_endpoint: "udp://relay.example.net:4500".to_string(),
            ..LineVowifiConfig::default()
        };
        assert_eq!(
            build_live_network_overrides(&config).unwrap_err(),
            "vowifi_proxy_mode_not_implemented:udp_relay"
        );
    }

    #[test]
    fn line_network_overrides_accept_socks5_and_reject_bad_endpoints() {
        let good = LineVowifiConfig {
            proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
            proxy_endpoint: "socks5://user:pass@127.0.0.1:1080".to_string(),
            ..LineVowifiConfig::default()
        };
        let overrides = build_live_network_overrides(&good).expect("socks5 accepted");
        assert!(matches!(overrides.proxy, Some(LiveProxySetting::Socks5(_))));

        // A malformed endpoint must be rejected at configuration time, not at
        // connect time.
        let bad = LineVowifiConfig {
            proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
            proxy_endpoint: "socks5://missing-port".to_string(),
            ..LineVowifiConfig::default()
        };
        assert!(build_live_network_overrides(&bad).is_err());
    }

    #[test]
    fn per_line_overrides_do_not_leak_between_lines() {
        // The whole point of keying overrides by line: two SIMs on different
        // operators, each with its own proxy and DNS, must stay independent.
        let japan = LineVowifiConfig {
            enabled: true,
            proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
            proxy_endpoint: "socks5://127.0.0.1:1080".to_string(),
            dns_server: "1.1.1.1".to_string(),
            profile_id: Some("jp_carrier".to_string()),
            ..LineVowifiConfig::default()
        };
        let malaysia = LineVowifiConfig {
            enabled: true,
            proxy_mode: VowifiProxyMode::Direct,
            proxy_endpoint: String::new(),
            dns_server: "8.8.8.8".to_string(),
            profile_id: Some("my_carrier".to_string()),
            ..LineVowifiConfig::default()
        };

        configure_live_network_overrides("line-jp", &japan).expect("configure jp");
        configure_live_network_overrides("line-my", &malaysia).expect("configure my");

        let jp = line_overrides("line-jp");
        let my = line_overrides("line-my");
        assert_eq!(jp.profile_id.as_deref(), Some("jp_carrier"));
        assert_eq!(my.profile_id.as_deref(), Some("my_carrier"));
        assert_eq!(
            jp.dns_server.map(|ip| ip.to_string()).as_deref(),
            Some("1.1.1.1")
        );
        assert_eq!(
            my.dns_server.map(|ip| ip.to_string()).as_deref(),
            Some("8.8.8.8")
        );
        // Only the Japanese line is proxied.
        assert!(jp.proxy.is_some());
        assert!(my.proxy.is_none());

        // An unknown line falls back to profile defaults rather than borrowing
        // another line's settings.
        assert_eq!(
            line_overrides("line-unknown"),
            LiveNetworkOverrides::default()
        );

        forget_live_network_overrides("line-jp");
        assert_eq!(line_overrides("line-jp"), LiveNetworkOverrides::default());
        // Forgetting one line must not disturb the other.
        assert_eq!(
            line_overrides("line-my").profile_id.as_deref(),
            Some("my_carrier")
        );
        forget_live_network_overrides("line-my");
    }

    fn register_variant(label: &str) -> LiveRegisterHeaderVariant {
        *LIVE_REGISTER_HEADER_VARIANTS
            .iter()
            .find(|variant| variant.label == label)
            .expect("register variant exists")
    }

    fn ee_register_variant(label: &str) -> LiveRegisterHeaderVariant {
        *GB_EE_REGISTER_HEADER_VARIANTS
            .iter()
            .find(|variant| variant.label == label)
            .expect("EE register variant exists")
    }
    use crate::connectivity::modems::softstack::vowifi::{
        profiles::{GB_EE_23433, NL_VODAFONE_20404},
        transport::{choose_route_policy, ResolvedEpdgEndpoint},
    };
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[derive(Debug, Clone, Copy)]
    struct MockReadyAdapter;

    impl LiveStageAdapter for MockReadyAdapter {
        fn run_stage<'a>(
            &'a self,
            stage: ExecutorStage,
            _profile: &'static CarrierProfile,
        ) -> LiveAdapterFuture<'a> {
            Box::pin(async move {
                Ok(LiveStageObservation {
                    stage: stage.as_str(),
                    ready: true,
                    detail: "mock_stage_ready",
                    sensitive_values_policy: "metadata_only",
                })
            })
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct MockEpdgAdapter;

    impl LiveEpdgAdapter for MockEpdgAdapter {
        fn resolve_epdg<'a>(
            &'a self,
            profile: &'static CarrierProfile,
        ) -> Pin<Box<dyn Future<Output = Result<ResolvedEpdgEndpoint, LiveStageError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(ResolvedEpdgEndpoint {
                    host: profile.epdg.host.to_string(),
                    port: profile.epdg.port,
                    addresses: vec![SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10)),
                        profile.epdg.port,
                    )],
                    route_policy: choose_route_policy(&profile.meta, profile.epdg.host, None),
                })
            })
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct MockDatagramAdapter;

    impl LiveDatagramAdapter for MockDatagramAdapter {
        fn check_udp_path<'a>(
            &'a self,
            _stage: ExecutorStage,
            _profile: &'static CarrierProfile,
        ) -> Pin<Box<dyn Future<Output = Result<(), LiveStageError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn enabled_gate() -> LiveExecutorGateReport {
        LiveExecutorGateReport {
            live_network_authorized: true,
            device_state_changes_authorized: true,
            adb_path_configured: true,
            device_admin_url_configured: true,
            implementation_ready: true,
            effective_live_network_allowed: true,
            effective_device_state_changes_allowed: true,
            blockers: Vec::new(),
            sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
        }
    }

    #[tokio::test]
    async fn blocked_gate_skips_without_running_adapter() {
        let runner = LiveStageRunner::new(
            LiveExecutorGateReport::disabled(),
            &GB_EE_23433,
            MockReadyAdapter,
        );
        let result = runner
            .run(ExecutorStageRequest {
                stage: ExecutorStage::Epdg,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "blocked".to_string(),
                line_id: String::new(),
            })
            .await;

        assert_eq!(result.status, "skipped");
        assert_eq!(
            result.reason.as_deref(),
            Some("live_network_executor_disabled")
        );
    }

    #[tokio::test]
    async fn mock_adapter_completes_enabled_stage_without_sensitive_values() {
        let runner = LiveStageRunner::new(enabled_gate(), &GB_EE_23433, MockReadyAdapter);
        let result = runner
            .run(ExecutorStageRequest {
                stage: ExecutorStage::Epdg,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "mock".to_string(),
                line_id: String::new(),
            })
            .await;

        assert_eq!(result.status, "completed");
        assert_eq!(result.reason, None);
        assert_eq!(
            result
                .soak_observation
                .as_ref()
                .map(|observation| observation.metric_name),
            Some("epdg_resolution_attempts")
        );

        let json = serde_json::to_string(&result).expect("serialize result");
        for forbidden_key in ["imsi", "iccid", "imei", "eid", "key_material", "token"] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[tokio::test]
    async fn network_adapter_completes_epdg_and_datagram_stages_with_mock_io() {
        let adapter = LiveNetworkStageAdapter::new(MockEpdgAdapter, MockDatagramAdapter);
        let runner = LiveStageRunner::new(enabled_gate(), &GB_EE_23433, adapter);

        let epdg_result = runner
            .run(ExecutorStageRequest {
                stage: ExecutorStage::Epdg,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "network-mock".to_string(),
                line_id: String::new(),
            })
            .await;

        assert_eq!(epdg_result.status, "completed");
        assert_eq!(epdg_result.reason, None);

        let ike_result = runner
            .run(ExecutorStageRequest {
                stage: ExecutorStage::Ike,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "network-mock".to_string(),
                line_id: String::new(),
            })
            .await;

        assert_eq!(ike_result.status, "completed");
        assert_eq!(ike_result.reason, None);
    }

    #[test]
    fn status_probe_depth_uses_single_sa_init_candidate() {
        assert_eq!(LIVE_IKE_TRANSPORT_PATHS[..1][0].destination_port, IKE_PORT);
        assert!(!LIVE_IKE_TRANSPORT_PATHS[..1][0].initial_nat_t);
    }

    #[test]
    fn full_handshake_covers_all_common_epdg_addresses_before_failing() {
        assert_eq!(LIVE_IKE_MAX_ENDPOINTS_PER_PASS, 5);
        assert_eq!(LIVE_IKE_MAX_TRANSPORT_PATHS_PER_PASS, 2);
        assert_eq!(LIVE_IKE_MAX_PROPOSAL_GROUPS_PER_PASS, 2);
    }

    #[tokio::test]
    async fn status_probe_adapter_rejects_non_ike_stages() {
        let err = StatusProbeDatagramAdapter
            .check_udp_path(ExecutorStage::ChildSa, &GB_EE_23433)
            .await
            .expect_err("status probe should only cover IKE readiness");

        assert_eq!(err.reason, "status_probe_stage_not_supported");
    }

    #[tokio::test]
    async fn local_bind_can_choose_ephemeral_source_port_for_nat_paths() {
        let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 500);
        let local = local_bind_addr_for_destination(remote, 0)
            .await
            .expect("ephemeral local bind address");

        assert_ne!(local.port(), 0);
        assert_eq!(local.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn hmac_md5_matches_public_test_vector() {
        let digest = hmac_md5(&[0x0b; 16], b"Hi There");
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        assert_eq!(hex, "9294727a3638bb1c13f48ef8158bfc9d");
    }

    #[test]
    fn gb_ee_register_variants_cover_bounded_clean_room_shapes() {
        let variants = live_register_header_variants(&GB_EE_23433);

        assert_eq!(variants.len(), 9);
        assert!(variants.iter().any(|variant| variant.initial_authorization
            == LiveInitialAuthorizationFormat::AkaEmptyUriFirst));
        assert!(variants
            .iter()
            .any(|variant| variant.initial_authorization == LiveInitialAuthorizationFormat::None));
        assert!(variants.iter().any(|variant| variant.initial_authorization
            == LiveInitialAuthorizationFormat::AkaZeroResponseUriFirst));
        assert!(variants
            .iter()
            .any(|variant| variant.force_sec_agree_headers));
        assert!(variants.iter().any(|variant| matches!(
            variant.identity_format,
            LiveRegisterIdentityFormat::PrefixedImsiHomeDomain
        )));
        assert!(variants.iter().any(|variant| matches!(
            variant.identity_format,
            LiveRegisterIdentityFormat::ImsiPhoneUri
        )));
        assert!(variants.iter().any(|variant| matches!(
            variant.identity_format,
            LiveRegisterIdentityFormat::MsisdnPhoneUri
        )));
    }

    #[test]
    fn standard_profiles_use_standard_register_variants() {
        let variants = live_register_header_variants(&NL_VODAFONE_20404);

        assert_eq!(variants.len(), LIVE_REGISTER_HEADER_VARIANTS.len());
        assert!(variants
            .iter()
            .any(|variant| variant.label == "ims_features_aka_uri_first_full_sec_client"));
    }

    #[test]
    fn live_runtime_config_defaults_to_openstick_qmi_environment() {
        let config = config_from_pairs(&[]);

        assert_eq!(config.qmi_proxy_socket, DEFAULT_QMI_PROXY_SOCKET);
        assert_eq!(config.qmi_device, DEFAULT_QMI_DEVICE);
        assert_eq!(config.uim_slot, DEFAULT_UIM_SLOT);
        assert_eq!(config.tun_name, DEFAULT_LIVE_TUN_NAME);
        assert_eq!(config.ims_security_port_c, LIVE_IMS_SECURITY_PORT_C);
        assert_eq!(config.ims_security_port_s, LIVE_IMS_SECURITY_PORT_S);
    }

    #[test]
    fn live_runtime_config_accepts_non_sensitive_env_overrides() {
        let config = config_from_pairs(&[
            (ENV_QMI_PROXY_SOCKET, "@alt-qmi-proxy"),
            (ENV_QMI_DEVICE, "/dev/cdc-wdm0"),
            (ENV_UIM_SLOT, "2"),
            (ENV_TUN_NAME, "sa_vwf1"),
            (ENV_IMS_SECURITY_PORT_C, "6064"),
            (ENV_IMS_SECURITY_PORT_S, "6063"),
        ]);

        assert_eq!(config.qmi_proxy_socket, "@alt-qmi-proxy");
        assert_eq!(config.qmi_device, "/dev/cdc-wdm0");
        assert_eq!(config.uim_slot, 2);
        assert_eq!(config.tun_name, "sa_vwf1");
        assert_eq!(config.ims_security_port_c, 6064);
        assert_eq!(config.ims_security_port_s, 6063);
    }

    #[test]
    fn live_runtime_config_rejects_empty_or_zero_overrides() {
        let config = config_from_pairs(&[
            (ENV_QMI_PROXY_SOCKET, " "),
            (ENV_QMI_DEVICE, ""),
            (ENV_UIM_SLOT, "0"),
            (ENV_TUN_NAME, " "),
            (ENV_IMS_SECURITY_PORT_C, "0"),
            (ENV_IMS_SECURITY_PORT_S, "not-a-port"),
        ]);

        assert_eq!(config.qmi_proxy_socket, DEFAULT_QMI_PROXY_SOCKET);
        assert_eq!(config.qmi_device, DEFAULT_QMI_DEVICE);
        assert_eq!(config.uim_slot, DEFAULT_UIM_SLOT);
        assert_eq!(config.tun_name, DEFAULT_LIVE_TUN_NAME);
        assert_eq!(config.ims_security_port_c, LIVE_IMS_SECURITY_PORT_C);
        assert_eq!(config.ims_security_port_s, LIVE_IMS_SECURITY_PORT_S);
    }

    fn config_from_pairs(pairs: &[(&'static str, &'static str)]) -> LiveRuntimeConfig {
        LiveRuntimeConfig::from_lookup(|key| {
            pairs
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, value)| (*value).to_string())
        })
    }

    #[test]
    fn akav2_md5_digest_uses_res_ik_ck_without_serializing_values() {
        let nonce = BASE64_STANDARD.encode([0x11u8; 32]);
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"{}\", algorithm=AKAv2-MD5, nonce=\"{}\", qop=\"auth\"\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            GB_EE_23433.ims.realm, nonce
        );
        let challenge = parse_live_digest_challenge(&response, GB_EE_23433.ims.realm)
            .expect("parse AKAv2-MD5 challenge");
        let aka = crate::connectivity::modems::softstack::vowifi::qmi_uim::UsimAkaApduResult {
            res: vec![0x22; 8],
            ck: vec![0x33; 16],
            ik: vec![0x44; 16],
            auts: None,
        };
        let proof = compute_aka_md5_response(
            "redacted@ims.example",
            GB_EE_23433.ims.realm,
            &aka,
            &challenge.algorithm,
            "REGISTER",
            "sip:ims.example",
            &challenge.nonce,
            challenge.qop,
            "abcdef0123456789",
        )
        .expect("compute AKAv2-MD5 proof");

        assert_eq!(challenge.algorithm, "AKAv2-MD5");
        assert_eq!(proof.len(), 32);
        assert!(proof.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn md5_digest_challenge_can_use_usim_res_as_one_time_password() {
        let nonce = BASE64_STANDARD.encode([0x55u8; 32]);
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"{}\", algorithm=MD5, nonce=\"{}\", qop=\"auth\"\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            GB_EE_23433.ims.realm, nonce
        );
        let challenge = parse_live_digest_challenge(&response, GB_EE_23433.ims.realm)
            .expect("parse MD5 challenge");
        let aka = crate::connectivity::modems::softstack::vowifi::qmi_uim::UsimAkaApduResult {
            res: vec![0x66; 8],
            ck: vec![0x77; 16],
            ik: vec![0x88; 16],
            auts: None,
        };
        let proof = compute_aka_md5_response(
            "redacted@ims.example",
            GB_EE_23433.ims.realm,
            &aka,
            &challenge.algorithm,
            "REGISTER",
            "sip:ims.example",
            &challenge.nonce,
            challenge.qop,
            "abcdef0123456789",
        )
        .expect("compute MD5 proof");

        assert_eq!(challenge.algorithm, "MD5");
        assert_eq!(proof.len(), 32);
    }

    #[test]
    fn ee_policy_rejects_short_plain_md5_register_challenge() {
        let nonce = BASE64_STANDARD.encode([0x44u8; 16]);
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"{}\", algorithm=MD5, nonce=\"{}\", qop=\"auth\"\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            GB_EE_23433.ims.realm, nonce
        );

        let challenge = parse_live_digest_challenge(&response, GB_EE_23433.ims.realm)
            .expect("parse short plain MD5 challenge");
        let err = reject_plain_digest_when_disabled(&GB_EE_23433, &challenge)
            .expect_err("plain MD5 should be blocked by profile policy");

        assert_eq!(challenge.nonce_kind, LiveDigestNonceKind::PlainDigest);
        assert_eq!(err.reason, "ims_digest_plain_md5_disabled");
    }

    #[test]
    fn digest_nonce_decoder_prefers_hex_for_ascii_hex_challenges() {
        let nonce = "0123456789abcdeffedcba9876543210";

        let decoded = decode_digest_nonce(nonce).expect("decode ascii hex nonce");

        assert_eq!(
            decoded,
            vec![
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
                0x32, 0x10
            ]
        );
    }

    #[test]
    fn ee_register_requests_offer_security_client_without_forcing_sec_agree_headers() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let initial = context.build_initial_request(
            &GB_EE_23433,
            register_variant("profile_default_spaced_sec_client"),
        );
        assert!(initial.contains("Supported: path,sec-agree,gruu\r\n"));
        assert!(!initial.contains("Require: sec-agree\r\n"));
        assert!(!initial.contains("Proxy-Require: sec-agree\r\n"));
        assert!(initial.contains("Security-Client: ipsec-3gpp;"));
        assert!(initial.contains("; alg=hmac-sha-1-96;"));
        assert!(initial.contains("; ealg=aes-cbc;"));
        assert!(initial.contains("; prot=esp;"));
        assert!(initial.contains("; mod=trans;"));
        assert!(initial.contains("; spi-c="));
        assert!(initial.contains("; spi-s="));
        assert!(initial.contains("; port-c=5064; port-s=5063"));
        assert!(initial.contains("Route: <sip:[::1]:5060;lr>\r\n"));
        assert!(initial.contains("+g.3gpp.accesstype=\"IEEE-802.11\""));
        assert!(initial.contains("+g.3gpp.smsip"));
        assert!(!initial.contains("+sip.instance="));
        assert!(!initial.contains(";reg-id="));
        assert!(initial.contains(
            "P-Access-Network-Info: IEEE-802.11;i-wlan-node-id=000000000000;network-provided\r\n"
        ));

        let authenticated = context.build_authenticated_request(
            &GB_EE_23433,
            register_variant("profile_default_spaced_sec_client"),
            "Authorization: Digest username=\"redacted\",realm=\"ims.example\",nonce=\"redacted\",uri=\"sip:ims.example\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
            Some("ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans"),
        );
        assert!(!authenticated.contains("Require: sec-agree\r\n"));
        assert!(!authenticated.contains("Proxy-Require: sec-agree\r\n"));
        assert!(authenticated.contains("Security-Verify: ipsec-3gpp;"));
    }

    #[test]
    fn register_reuses_security_client_offer_across_initial_and_authenticated_requests() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");
        let variant = ee_register_variant("gb_ee_aka_zero_sec_client");

        let initial = context.build_initial_request(&GB_EE_23433, variant);
        let authenticated = context.build_authenticated_request(
            &GB_EE_23433,
            variant,
            "Authorization: Digest username=\"redacted\",realm=\"ims.example\",nonce=\"redacted\",uri=\"sip:ims.example\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
            Some("ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans"),
        );

        assert_eq!(
            sip_header_values(&initial, "security-client"),
            sip_header_values(&authenticated, "security-client")
        );
        assert!(
            initial.contains("response=\"00000000000000000000000000000000\",realm=\"ims.mnc033.mcc234.3gppnetwork.org\",nonce=\"\"")
        );
    }

    #[test]
    fn register_can_offer_minimal_spaced_security_client_for_strict_pcscf_parsers() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let initial = context
            .build_initial_request(&GB_EE_23433, register_variant("ims_features_plain_pani"));

        assert!(initial.contains("Security-Client: ipsec-3gpp; alg=hmac-sha-1-96; ealg=aes-cbc;"));
        assert!(initial.contains("; spi-c="));
        assert!(initial.contains("; spi-s="));
        assert!(initial.contains("; port-c=5064; port-s=5063"));
        assert!(!initial.contains("; prot=esp;"));
        assert!(!initial.contains("; mod=trans;"));
    }

    #[test]
    fn phone_uri_identity_keeps_private_identity_separate_from_public_aor() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example;user=phone".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: true,
                },
                shape: "imsi_phone_uri",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let request = context.build_initial_request(
            &GB_EE_23433,
            register_variant("phone_uri_identity_ims_features"),
        );

        assert!(request.contains("From: <sip:001010123456789@ims.example;user=phone>;tag="));
        assert!(request.contains("To: <sip:001010123456789@ims.example;user=phone>\r\n"));
        assert!(request
            .contains("P-Preferred-Identity: <sip:001010123456789@ims.example;user=phone>\r\n"));
        assert!(
            request.contains("Contact: <sip:001010123456789@[::1]:5060;user=phone;transport=tcp>")
        );
    }

    #[test]
    fn strict_profiles_can_require_sec_agree_when_policy_says_so() {
        let context = LiveRegisterRequestContext::new(
            &crate::connectivity::modems::softstack::vowifi::profiles::NL_VODAFONE_20404,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let initial = context.build_initial_request(
            &crate::connectivity::modems::softstack::vowifi::profiles::NL_VODAFONE_20404,
            register_variant("profile_default_spaced_sec_client"),
        );

        assert!(initial.contains("Require: sec-agree\r\n"));
        assert!(initial.contains("Proxy-Require: sec-agree\r\n"));
        assert!(initial.contains("Security-Client: ipsec-3gpp;"));
    }

    #[test]
    fn register_header_variants_can_force_sec_agree_or_omit_route() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let forced = context.build_initial_request(
            &GB_EE_23433,
            register_variant("sec_agree_required_spaced_sec_client"),
        );
        assert!(forced.contains("Require: sec-agree\r\n"));
        assert!(forced.contains("Proxy-Require: sec-agree\r\n"));
        assert!(forced.contains("Route: <sip:[::1]:5060;lr>\r\n"));

        let routeless = context.build_initial_request(
            &GB_EE_23433,
            register_variant("route_omitted_spaced_sec_client"),
        );
        assert!(!routeless.contains("Route: <sip:[::1]:5060;lr>\r\n"));
        assert!(!routeless.contains("Require: sec-agree\r\n"));
    }

    #[test]
    fn register_can_probe_pcscf_socket_request_uri_without_route_header() {
        let context = LiveRegisterRequestContext::new(
            &GB_EE_23433,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "001010123456789@ims.example".to_string(),
                    public_uri: "sip:001010123456789@ims.example".to_string(),
                    contact_user: "001010123456789".to_string(),
                    home_domain: "ims.example".to_string(),
                    contact_user_phone: false,
                },
                shape: "fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let request = context.build_initial_request(
            &GB_EE_23433,
            LiveRegisterHeaderVariant {
                label: "pcscf_uri_unit_test",
                force_sec_agree_headers: false,
                include_route_header: false,
                include_security_client: true,
                initial_authorization: LiveInitialAuthorizationFormat::None,
                security_client_format: LiveSecurityClientFormat::FullSpaced,
                request_uri: LiveRegisterRequestUri::PcscfSocket,
                identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
                header_profile: LiveRegisterHeaderProfile::DEFAULT,
            },
        );

        assert!(request.starts_with("REGISTER sip:[::1]:5060 SIP/2.0\r\n"));
        assert!(!request.contains("Route: <sip:[::1]:5060;lr>\r\n"));
    }

    #[test]
    fn pcscf_candidates_keep_inner_family_and_deduplicate_addresses() {
        let inner = IpAddr::V6(Ipv6Addr::LOCALHOST);
        let configuration = IkeConfigurationMaterial {
            assigned_inner_addresses: vec![inner],
            assigned_ipv6_prefix_length: Some(64),
            pcscf_addresses: vec![
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ],
            dns_addresses: vec![],
        };

        let addrs = pcscf_candidates(&GB_EE_23433, &configuration, inner);

        assert_eq!(addrs, vec![IpAddr::V6(Ipv6Addr::LOCALHOST)]);
    }

    #[test]
    fn digest_challenge_parser_prefers_aka_when_plain_md5_appears_first() {
        let short_nonce = BASE64_STANDARD.encode([0x10u8; 24]);
        let aka_nonce = BASE64_STANDARD.encode([0x20u8; 32]);
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"plain.example\", algorithm=MD5, nonce=\"{}\", qop=\"auth\", ",
                "Digest realm=\"{}\", algorithm=AKAv2-MD5, nonce=\"{}\", qop=\"auth\"\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            short_nonce, GB_EE_23433.ims.realm, aka_nonce
        );

        let challenge = parse_live_digest_challenge(&response, GB_EE_23433.ims.realm)
            .expect("parse AKA challenge after plain MD5 challenge");

        assert_eq!(challenge.algorithm, "AKAv2-MD5");
        assert_eq!(challenge.realm, GB_EE_23433.ims.realm);
        assert_eq!(challenge.rand, vec![0x20; 16]);
        assert_eq!(challenge.autn, vec![0x20; 16]);
    }

    #[test]
    fn digest_challenge_splitter_ignores_commas_inside_quoted_params() {
        let values = split_digest_challenge_values(
            "Digest realm=\"one\", qop=\"auth,auth-int\", nonce=\"a\", Digest realm=\"two\", nonce=\"b\"",
        );

        assert_eq!(values.len(), 2);
        assert!(values[0].contains("qop=\"auth,auth-int\""));
        assert!(values[1].starts_with("Digest realm=\"two\""));
    }

    #[test]
    fn sip_frame_len_splits_coalesced_tcp_frames() {
        let first = b"SIP/2.0 202 Accepted\r\nContent-Length: 0\r\n\r\n";
        let second = b"MESSAGE sip:redacted@example SIP/2.0\r\nContent-Length: 2\r\n\r\n\x01\x02";
        let mut combined = Vec::new();
        combined.extend_from_slice(first);
        combined.extend_from_slice(second);

        let first_len = sip_complete_frame_len(&combined).expect("first frame complete");

        assert_eq!(first_len, first.len());
        assert!(sip_frame_is_request(&combined[first_len..], "MESSAGE"));
        assert_eq!(
            sip_complete_frame_len(&combined[first_len..]),
            Some(second.len())
        );
    }

    #[test]
    fn sms_session_refresh_retry_is_limited_to_pre_send_or_auth_failures() {
        for reason in [
            "ims_tcp_connect_failed",
            "ims_tcp_connect_timeout",
            "ims_tcp_bind_preferred_port_failed",
            "sms_tcp_local_addr_unavailable",
            "sms_message_sip_401",
            "sms_message_sip_503",
        ] {
            assert!(
                live_sms_session_refresh_retryable(reason),
                "{reason} should refresh IMS session"
            );
        }

        for reason in [
            "sms_message_response_timeout",
            "sms_message_write_failed",
            "sms_message_ack_timeout",
            "sms_message_sip_202",
            "sms_message_sip_404",
        ] {
            assert!(
                !live_sms_session_refresh_retryable(reason),
                "{reason} should not risk a duplicate MESSAGE retry"
            );
        }
    }

    #[test]
    fn sms_route_variants_only_retry_after_sip_rejections() {
        for reason in [
            "sms_message_sip_401",
            "sms_message_sip_403",
            "sms_message_sip_404",
            "sms_message_sip_503",
        ] {
            assert!(
                live_sms_route_variant_retryable(reason),
                "{reason} should allow trying another MESSAGE URI shape"
            );
        }

        for reason in [
            "ims_tcp_connect_timeout",
            "ims_tcp_connect_failed",
            "sms_message_response_timeout",
            "sms_message_write_failed",
            "sms_message_ack_timeout",
            "sms_tcp_local_addr_unavailable",
        ] {
            assert!(
                !live_sms_route_variant_retryable(reason),
                "{reason} should not resend the same MESSAGE through another URI variant"
            );
        }
    }

    #[test]
    fn ims_register_cache_ttl_uses_network_expires_with_safety_skew() {
        assert_eq!(
            live_ims_register_cache_ttl(Some(3600)),
            Duration::from_secs(3540)
        );
        assert_eq!(
            live_ims_register_cache_ttl(Some(120)),
            LIVE_IMS_REGISTER_DEFAULT_TTL
        );
        assert_eq!(
            live_ims_register_cache_ttl(Some(7200)),
            LIVE_IMS_REGISTER_MAX_TTL
        );
        assert_eq!(
            live_ims_register_cache_ttl(None),
            LIVE_IMS_REGISTER_DEFAULT_TTL
        );
    }

    #[tokio::test]
    async fn ims_register_success_variant_is_prioritized_on_next_attempt() {
        clear_all_live_runtime().await;
        let success = ee_register_variant("gb_ee_aka_uri_first_required_sec_agree");

        record_live_ims_register_success_variant("line-test", &GB_EE_23433, success).await;
        let variants = live_register_header_variants_for_attempt("line-test", &GB_EE_23433).await;

        assert_eq!(
            variants.first().map(|variant| variant.label),
            Some(success.label)
        );
        assert_eq!(variants.len(), GB_EE_REGISTER_HEADER_VARIANTS.len());
        assert_eq!(
            variants
                .iter()
                .filter(|variant| variant.label == success.label)
                .count(),
            1
        );
        clear_all_live_runtime().await;
    }

    #[test]
    fn sms_send_total_timeout_stays_inside_http_live_budget() {
        assert!(LIVE_SMS_SEND_TOTAL_TIMEOUT < Duration::from_secs(90));
        assert!(
            LIVE_SMS_SEND_TOTAL_TIMEOUT >= LIVE_IMS_TCP_TIMEOUT + LIVE_IMS_REGISTER_READ_TIMEOUT
        );
    }
}
