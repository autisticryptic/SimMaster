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

#[cfg(test)]
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;

use super::{
    channel::{SipChannel, SipChannelSocket},
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
    ike_state::{IkeAccessConfig, IkeAuthProgress, IkeConfigurationMaterial, IkeStateMachine},
    ims,
    profiles::{self, CarrierProfile},
    qmi_uim::{
        execute_usim_authenticate_via_proxy_reason_with_retry,
        read_usim_epdg_config_via_proxy_reason,
        verify_usim_application_via_proxy_reason_with_retry, EpdgFqdnFormat, UsimEpdgAddress,
        UsimEpdgConfig, USIM_AID_PREFIX,
    },
    sms,
    transport::{
        choose_route_policy, ProxyKind, ResolvedEpdgEndpoint, TransportError,
        UdpSocketDatagramTransport,
    },
    tun_gateway::{
        self, ImsEspFlowConfig, ImsEspPolicyConfig, TunGatewayConfig, TunGatewayRuntime,
    },
    voice,
};
use crate::connectivity::core::{
    access::ImsChannel,
    access_network::{
        access_type_token, resolve_access_identity, sanitize_header_value, AccessIdentityPolicy,
        EpdgLocationSnapshot, ImsAccessNetworkContext, ImsAccessNetworkRuntime,
    },
    contact::{complete_contact_parameters, ContactCompletion},
    media::OperatorSocketCreator,
    register::{
        run_register_observed, status_is_terminal_register_failure, RegisterAuthenticator,
        RegisterFailure, RegisterTransactionKey, MAX_MIN_EXPIRES_ROUNDS,
        MAX_REGISTER_IGNORED_FRAMES, MAX_REGISTER_PROVISIONAL_RESPONSES, MIN_EXPIRES_CAP,
    },
    register_response::RegisterArtifacts,
    registration::{
        ImsRegistrationAccess, RegisteredImsContext, RegistrationLossReason,
        RegistrationRefreshResult,
    },
    sip_frame, ImsError,
};
use crate::connectivity::modems::ims::profile_override::SimOverride;
use crate::hardware::cellular::modem_manager::get_sim_info_for_modem_with_cache;
use crate::platform::config::{LineVowifiConfig, VowifiProxyMode};
use crate::services::supplementary::ut::{XcapAccessContext, XcapDigestProvider};
use crate::services::trunk::bridge::{
    DtmfCapabilities, DtmfSource, MediaOffer, OperatorCommand, OperatorEvent,
};
use crate::services::ue_worker::{UeSocket, UeSocketSpec, UeWorkerHandle};
use tokio::{
    net::TcpSocket,
    sync::{mpsc, Mutex},
};
use tracing::{debug, error, info, warn};

const LIVE_DNS_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_EPDG_MAX_HOST_CANDIDATES: usize = 8;
const LIVE_UICC_EPDG_CACHE_TTL: Duration = Duration::from_secs(300);

impl super::operator::MediaRouteInstaller for TunGatewayRuntime {
    fn ensure_media_route(&self, remote: IpAddr) -> Result<(), String> {
        TunGatewayRuntime::ensure_media_route(self, remote)
            .map_err(|error| error.reason().to_string())
    }
}
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
/// RFC 5626 `reg-id` for this access leg's flow.
///
/// Taken from the shared access policy rather than written literally, so the
/// WLAN and cellular legs cannot drift onto the same value. Both legs now
/// present one stable `+sip.instance`, and RFC 5626 §6 keys a binding on
/// (AOR, instance-id, reg-id) -- equal reg-ids would make whichever leg
/// registers second silently *replace* the other's binding (§3.2).
const WLAN_REG_ID: u32 = crate::connectivity::core::ims_access::ImsAccess::Wlan.reg_id();
const DEFAULT_QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const DEFAULT_LIVE_TUN_NAME: &str = "sa_vwf0";
const LIVE_IMS_TCP_TIMEOUT: Duration = Duration::from_secs(8);
const LIVE_IMS_REGISTER_READ_TIMEOUT: Duration = Duration::from_secs(8);
/// A refresh is a lease-maintenance operation, not a reason to tear down a
/// healthy ePDG/IKE/ESP access leg after the first transient SIP failure. Keep
/// the old access path alive for two failed refresh cycles and rebuild it only
/// on the third consecutive cycle.
pub(crate) const LIVE_IMS_REFRESH_REBUILD_FAILURES: u8 = 3;
/// Shorter budget for alternate ESP policy candidates: if the first mapping
/// was silently dropped, the alternate is a probe and a long wait only stalls
/// the whole registration sweep.
const LIVE_IMS_REGISTER_CANDIDATE_READ_TIMEOUT: Duration = Duration::from_secs(4);
const LIVE_IMS_REGISTER_DEFAULT_TTL: Duration = Duration::from_secs(300);
const LIVE_IMS_REGISTER_MAX_TTL: Duration = Duration::from_secs(3600);
/// Global bound for static and response-driven REGISTER shapes on one P-CSCF.
/// The dynamic ladder is monotonic, but the cap remains a safety valve against
/// future carrier-specific candidates accidentally creating a retry cycle.
const LIVE_IMS_REGISTER_MAX_VARIANT_ATTEMPTS: usize = 8;
const LIVE_SMS_SEND_TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_SMS_FOLLOWUP_WINDOW: Duration = Duration::from_secs(20);
const LIVE_VOICE_INVITE_TOTAL_TIMEOUT: Duration = Duration::from_secs(32);
const LIVE_VOICE_MMTEL_ICSI: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
const LIVE_IMS_SECURITY_PORT_C: u16 = 5064;
const LIVE_IMS_SECURITY_PORT_S: u16 = 5063;
const IMS_MMTEL_ICSI_REF: &str = "urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel";
const ENV_QMI_PROXY_SOCKET: &str = "SIMADMIN_VOWIFI_QMI_PROXY_SOCKET";
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
static LIVE_IMS_CHANNEL: OnceLock<Mutex<HashMap<String, LiveImsChannel>>> = OnceLock::new();
static LIVE_XCAP_BINDING: OnceLock<Mutex<HashMap<String, LiveXcapBinding>>> = OnceLock::new();
static LIVE_IMS_REGISTER_SUCCESS_VARIANT: OnceLock<
    Mutex<HashMap<String, LiveImsRegisterSuccessVariant>>,
> = OnceLock::new();
/// Consecutive VoWiFi REGISTER *refresh-cycle* failures, keyed by line. This
/// is deliberately separate from the registration cache: the cache expires at
/// the proactive refresh deadline, while this state must survive the failed
/// attempts that follow that deadline without being mistaken for a new access
/// connection.
static LIVE_IMS_REFRESH_FAILURE: OnceLock<Mutex<HashMap<String, LiveImsRefreshFailure>>> =
    OnceLock::new();
/// Per-line network overrides, keyed by `line_id`.
///
/// Deliberately a map rather than a single value: each SIM may sit on a different
/// operator in a different country and needs its own ePDG, DNS and proxy. A global
/// override would make configuring line A silently change line B.
static LIVE_NETWORK_OVERRIDES: OnceLock<StdRwLock<HashMap<String, LiveNetworkOverrides>>> =
    OnceLock::new();
static LIVE_UICC_EPDG_CONFIG: OnceLock<StdRwLock<HashMap<String, CachedLiveUiccEpdgConfig>>> =
    OnceLock::new();

#[derive(Debug, Clone)]
struct CachedLiveUiccEpdgConfig {
    device: LiveSimDevice,
    loaded_at: Instant,
    config: UsimEpdgConfig,
}

fn live_uicc_epdg_config_cache() -> &'static StdRwLock<HashMap<String, CachedLiveUiccEpdgConfig>> {
    LIVE_UICC_EPDG_CONFIG.get_or_init(|| StdRwLock::new(HashMap::new()))
}

fn forget_live_uicc_epdg_config(line_id: &str) {
    live_uicc_epdg_config_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(line_id);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LiveNetworkOverrides {
    /// Pin this line to a specific carrier profile by `profile_id`. `None`
    /// resolves the profile automatically from the SIM's IMSI. A pinned id is
    /// strict and must never be replaced by a standard-derived fallback.
    profile_id: Option<String>,
    dns_servers: Vec<SocketAddr>,
    epdg_host: Option<String>,
    epdg_port: Option<u16>,
    epdg_apn: Option<String>,
    ip_stack: Option<String>,
    ims_domain: Option<String>,
    ims_realm: Option<String>,
    ims_registrar: Option<String>,
    ims_pcscf: Vec<String>,
    effective_device_imei: Option<String>,
    /// IMSI used for carrier matching and exposed IMS identities. The SIM
    /// reader identity remains separate and is still used for AKA APDUs.
    effective_imsi: Option<String>,
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

/// Validate a prospective connection snapshot without publishing it.
pub fn validate_live_network_overrides(
    config: &LineVowifiConfig,
    sim_override: Option<&SimOverride>,
) -> Result<(), String> {
    build_live_network_overrides(config, sim_override, None).map(|_| ())
}

/// Fix (or clear) the immutable network snapshot for a line at the start of a
/// new connection. Callers must not invoke this for an already-active session;
/// refresh and in-dialog operations intentionally keep reading the same value.
pub fn configure_live_network_overrides(
    line_id: &str,
    config: &LineVowifiConfig,
    sim_override: Option<&SimOverride>,
) -> Result<(), String> {
    configure_live_network_overrides_with_device_imei(line_id, config, sim_override, None)
}

pub fn configure_live_network_overrides_with_device_imei(
    line_id: &str,
    config: &LineVowifiConfig,
    sim_override: Option<&SimOverride>,
    device_imei: Option<&str>,
) -> Result<(), String> {
    if line_id.trim().is_empty() {
        return Err("line_id_required".to_string());
    }
    let next = build_live_network_overrides(config, sim_override, device_imei)?;
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

fn build_live_network_overrides(
    config: &LineVowifiConfig,
    sim_override: Option<&SimOverride>,
    device_imei: Option<&str>,
) -> Result<LiveNetworkOverrides, String> {
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
    let access = sim_override.map(|override_| &override_.ims_vowifi);
    let dns_servers = access
        .and_then(|access| access.dns.as_ref())
        .into_iter()
        .flatten()
        .map(|server| {
            super::profile_record::parse_dns_server(server)
                .ok_or_else(|| "vowifi_dns_server_invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LiveNetworkOverrides {
        dns_servers,
        proxy,
        profile_id: access
            .and_then(|access| access.profile_id.as_ref())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        epdg_host: non_empty_override(access.and_then(|access| access.epdg_host.as_ref())),
        epdg_port: access.and_then(|access| access.epdg_port),
        epdg_apn: non_empty_override(access.and_then(|access| access.apn.as_ref())),
        ip_stack: non_empty_override(access.and_then(|access| access.ip_stack.as_ref())),
        ims_domain: non_empty_override(access.and_then(|access| access.domain.as_ref())),
        ims_realm: non_empty_override(access.and_then(|access| access.realm.as_ref())),
        ims_registrar: non_empty_override(access.and_then(|access| access.registrar.as_ref())),
        ims_pcscf: access
            .and_then(|access| access.pcscf.as_ref())
            .cloned()
            .unwrap_or_default(),
        effective_device_imei:
            crate::connectivity::modems::ims::effective_profile::resolve_effective_device_identity(
                sim_override,
                device_imei,
            )
            .imei,
        effective_imsi: access
            .filter(|access| access.spoof_imsi)
            .and_then(|access| access.custom_imsi.as_ref())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

fn non_empty_override(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn line_overrides(line_id: &str) -> LiveNetworkOverrides {
    network_overrides_map()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned()
        .unwrap_or_default()
}

/// Return the IMSI used for VoWiFi carrier matching and network identities.
/// This is intentionally distinct from the modem SIM identity used for AKA.
pub fn effective_imsi_for_line(line_id: &str, modem_imsi: &str) -> String {
    line_overrides(line_id)
        .effective_imsi
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| modem_imsi.trim().to_string())
}

/// The carrier profile a line is pinned to, if any. Unknown lines and lines
/// without a pin return `None`.
pub fn line_pinned_profile_id(line_id: &str) -> Option<String> {
    line_overrides(line_id).profile_id
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveImsTarget {
    domain: String,
    realm: String,
    registrar: Option<String>,
    pcscf: Vec<String>,
}

fn live_ims_target(line_id: &str, profile: &CarrierProfile) -> LiveImsTarget {
    let overrides = line_overrides(line_id);
    LiveImsTarget {
        domain: overrides
            .ims_domain
            .unwrap_or_else(|| profile.ims.domain.to_string()),
        realm: overrides
            .ims_realm
            .unwrap_or_else(|| profile.ims.realm.to_string()),
        registrar: overrides
            .ims_registrar
            .or_else(|| profile.ims.registrar.map(str::to_string)),
        pcscf: if overrides.ims_pcscf.is_empty() {
            profile
                .ims
                .pcscf
                .map(|value| vec![value.to_string()])
                .unwrap_or_default()
        } else {
            overrides.ims_pcscf
        },
    }
}

fn live_ike_access(line_id: &str, profile: &CarrierProfile) -> IkeAccessConfig {
    live_ike_access_for_epdg(line_id, profile, None)
}

fn live_ike_access_for_epdg(
    line_id: &str,
    profile: &CarrierProfile,
    selected_epdg_host: Option<&str>,
) -> IkeAccessConfig {
    let overrides = line_overrides(line_id);
    let configured_host = overrides
        .epdg_host
        .clone()
        .unwrap_or_else(|| profile.epdg.host.to_string());
    // An IP address from EFePDGId is a transport destination, not a useful
    // FQDN-shaped IKE responder identity. Keep the explicit/profile host for
    // IDr in that case; a selected UICC/TAI FQDN becomes the actual IDr.
    let epdg_host = selected_epdg_host
        .and_then(|host| host.parse::<IpAddr>().err().map(|_| host.to_string()))
        .unwrap_or(configured_host);
    IkeAccessConfig {
        ip_stack: overrides
            .ip_stack
            .unwrap_or_else(|| profile.epdg.ip_stack.to_string()),
        apn: overrides
            .epdg_apn
            .or_else(|| profile.epdg.apn.map(str::to_string)),
        epdg_host,
        device_identity: profile
            .identity
            .device_identity_enabled
            .then_some(overrides.effective_device_imei)
            .flatten(),
    }
}

/// Resolve the carrier profile for a line, honoring its current SIM snapshot.
///
/// A pinned `profile_id` is tried first against catalog/local-database profiles;
/// Explicit pins are strict. Without a pin, identity matching prefers a
/// published database profile and then uses the marked standard-derived
/// fallback when no usable row exists.
pub fn resolve_profile_for_line(
    line_id: &str,
    imsi: &str,
    home_plmn: Option<&str>,
) -> Option<profiles::CarrierMatch> {
    if line_id.trim().is_empty() {
        return None;
    }
    let pinned = line_pinned_profile_id(line_id);
    let effective_imsi = effective_imsi_for_line(line_id, imsi);
    let effective_home_plmn = (effective_imsi == imsi.trim())
        .then_some(home_plmn)
        .flatten();
    profiles::resolve_for_line(
        pinned.as_deref(),
        effective_imsi.as_str(),
        effective_home_plmn,
    )
}

fn live_epdg_settings(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> (String, u16, Option<IpAddr>) {
    let overrides = line_overrides(line_id);
    (
        overrides
            .epdg_host
            .unwrap_or_else(|| profile.epdg.host.to_string()),
        overrides.epdg_port.unwrap_or(profile.epdg.port),
        live_dns_candidates(line_id, profile)
            .into_iter()
            .next()
            .map(|server| server.ip()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpdgCandidateSource {
    LineOverride,
    UiccSelection,
    UiccHomeIdentifier,
    VisitedCountryNaptr,
    CarrierProfile,
    HomePlmnDerived,
}

impl EpdgCandidateSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::LineOverride => "line_override",
            Self::UiccSelection => "uicc_selection",
            Self::UiccHomeIdentifier => "uicc_home_identifier",
            Self::VisitedCountryNaptr => "visited_country_naptr",
            Self::CarrierProfile => "carrier_profile",
            Self::HomePlmnDerived => "home_plmn_derived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpdgEndpointCandidate {
    address: UsimEpdgAddress,
    source: EpdgCandidateSource,
}

impl EpdgEndpointCandidate {
    fn host(&self) -> String {
        match &self.address {
            UsimEpdgAddress::Fqdn(host) => host.clone(),
            UsimEpdgAddress::Ip(ip) => ip.to_string(),
        }
    }
}

fn epdg_address_from_text(value: &str) -> Option<UsimEpdgAddress> {
    value
        .trim()
        .parse::<IpAddr>()
        .map(UsimEpdgAddress::Ip)
        .ok()
        .or_else(|| super::qmi_uim::normalize_epdg_fqdn(value).map(UsimEpdgAddress::Fqdn))
}

fn push_epdg_candidate(
    candidates: &mut Vec<EpdgEndpointCandidate>,
    address: UsimEpdgAddress,
    source: EpdgCandidateSource,
) {
    let address = match address {
        UsimEpdgAddress::Fqdn(host) => {
            let Some(host) = super::qmi_uim::normalize_epdg_fqdn(&host) else {
                return;
            };
            UsimEpdgAddress::Fqdn(host)
        }
        UsimEpdgAddress::Ip(ip) => UsimEpdgAddress::Ip(ip),
    };
    if candidates.len() >= LIVE_EPDG_MAX_HOST_CANDIDATES
        || candidates
            .iter()
            .any(|candidate| candidate.address == address)
    {
        return;
    }
    candidates.push(EpdgEndpointCandidate { address, source });
}

/// Result of the visited-country NAPTR discovery attempt.  Keeping an empty
/// DNS answer separate from a transport failure is important: an empty answer
/// means that visited-country ePDG selection is not mandatory, while a missing
/// DNS response terminates this selection procedure per TS 24.302.
#[derive(Debug, Clone, PartialEq, Eq)]
enum VisitedCountryNaptrState {
    NotQueried,
    EmptyResponse,
    Records(Vec<epdg::NaptrReplacement>),
    Failed(String),
}

fn canonical_plmn(plmn: &str) -> Option<String> {
    let plmn = plmn.trim();
    if !matches!(plmn.len(), 5 | 6) || !plmn.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if plmn.len() == 6 {
        Some(plmn.to_string())
    } else {
        // A two-digit MNC is represented as MCC + MNC2 by the modem/UICC, but
        // as MCC + 0 + MNC2 in a 3GPP Operator Identifier FQDN.
        Some(format!("{}0{}", &plmn[..3], &plmn[3..]))
    }
}

fn plmn_equivalent(left: &str, right: &str) -> bool {
    canonical_plmn(left) == canonical_plmn(right)
}

fn selection_entry_address(
    entry: &super::qmi_uim::UsimEpdgSelectionEntry,
    target_plmn: &str,
    location: Option<&EpdgLocationSnapshot>,
) -> Option<UsimEpdgAddress> {
    let canonical = canonical_plmn(target_plmn)?;
    let mcc = &canonical[..3];
    let mnc = &canonical[3..];
    let host = match entry.fqdn_format {
        EpdgFqdnFormat::OperatorIdentifier => profiles::standard_operator_epdg_fqdn(mcc, mnc),
        EpdgFqdnFormat::LocationBased => location
            .filter(|snapshot| plmn_equivalent(&snapshot.serving_plmn, &canonical))
            .and_then(|snapshot| {
                profiles::standard_tai_epdg_fqdn(mcc, mnc, snapshot.tac, &snapshot.technology)
            }),
    }?;
    epdg_address_from_text(&host)
}

fn serving_is_roaming(location: Option<&EpdgLocationSnapshot>, home_plmn: &str) -> bool {
    let Some(serving_plmn) = location.and_then(|snapshot| canonical_plmn(&snapshot.serving_plmn))
    else {
        return false;
    };
    canonical_plmn(home_plmn).is_some_and(|home_plmn| serving_plmn != home_plmn)
}

fn selection_entry_is_in_country(
    entry: &super::qmi_uim::UsimEpdgSelectionEntry,
    mcc: &str,
) -> bool {
    let pattern = entry.plmn_pattern.trim().as_bytes();
    let mcc = mcc.as_bytes();
    matches!(pattern.len(), 5 | 6)
        && mcc.len() == 3
        && pattern[..3]
            .iter()
            .zip(mcc)
            .all(|(pattern_digit, country_digit)| {
                *pattern_digit == b'D' || *pattern_digit == *country_digit
            })
}

fn concrete_selection_plmn(entry: &super::qmi_uim::UsimEpdgSelectionEntry) -> Option<String> {
    if entry.plmn_pattern.bytes().all(|byte| byte.is_ascii_digit()) {
        canonical_plmn(&entry.plmn_pattern)
    } else {
        None
    }
}

fn standard_epdg_record_plmn(record: &epdg::NaptrReplacement) -> Option<String> {
    profiles::parse_standard_operator_epdg_fqdn(&record.replacement)
        .map(|(mcc, mnc)| format!("{mcc}{mnc}"))
}

/// Build ordinary (non-emergency) ePDG candidates. A line override is strict.
/// Database/catalog profiles keep their explicit host and never gain a blind
/// public-DNS fallback. Standard-derived profiles may use TS 23.003 names, but
/// a tracking-area name is emitted only when EFePDGSelection explicitly asks
/// for the location-based format.
fn build_live_epdg_candidates(
    line_id: &str,
    profile: &'static CarrierProfile,
    uicc: &UsimEpdgConfig,
    location: Option<&EpdgLocationSnapshot>,
) -> Vec<EpdgEndpointCandidate> {
    build_live_epdg_candidates_with_naptr(
        line_id,
        profile,
        uicc,
        location,
        &VisitedCountryNaptrState::NotQueried,
    )
}

fn build_live_epdg_candidates_with_naptr(
    line_id: &str,
    profile: &'static CarrierProfile,
    uicc: &UsimEpdgConfig,
    location: Option<&EpdgLocationSnapshot>,
    visited_country: &VisitedCountryNaptrState,
) -> Vec<EpdgEndpointCandidate> {
    let overrides = line_overrides(line_id);
    if let Some(host) = overrides.epdg_host.as_deref() {
        return epdg_address_from_text(host)
            .map(|address| {
                vec![EpdgEndpointCandidate {
                    address,
                    source: EpdgCandidateSource::LineOverride,
                }]
            })
            .unwrap_or_default();
    }

    let mut candidates = Vec::new();
    let roaming = serving_is_roaming(location, profile.meta.plmn);
    let serving_plmn = location.and_then(|snapshot| canonical_plmn(&snapshot.serving_plmn));
    let home_plmn = canonical_plmn(profile.meta.plmn);
    // If no fresh serving snapshot exists, an Operator Identifier UICC row can
    // still be evaluated against the HPLMN. LocationBased rows intentionally
    // remain unusable until a fresh TAC/technology snapshot is available.
    let selection_plmn = serving_plmn.as_deref().or(home_plmn.as_deref());

    // EFePDGSelection applies in both home and roaming states. An exact
    // serving-PLMN row wins over Any_PLMN; preserving the selected rows before
    // adding the later fallbacks lets a failed ePDG/IKE attempt advance to the
    // UICC home identifier and finally the profile-derived host.
    let mut selected_entries = selection_plmn
        .map(|target_plmn| {
            uicc.selection
                .iter()
                .filter(|entry| !entry.is_any_plmn() && entry.matches_plmn(target_plmn))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if selected_entries.is_empty() {
        selected_entries = uicc
            .selection
            .iter()
            .filter(|entry| entry.is_any_plmn())
            .collect::<Vec<_>>();
    }
    selected_entries.sort_by_key(|entry| entry.priority);

    if let Some(target_plmn) = selection_plmn {
        for entry in selected_entries {
            if let Some(address) = selection_entry_address(entry, target_plmn, location) {
                push_epdg_candidate(&mut candidates, address, EpdgCandidateSource::UiccSelection);
            }
        }
    }

    // A selected EFePDGSelection row is authoritative for this PLMN. The
    // visited-country NAPTR procedure is only consulted when it did not yield a
    // directly usable UICC endpoint. Its failure is scoped to that discovery
    // step; it must not suppress a separately configured UICC home identifier
    // or carrier profile fallback.
    if roaming && candidates.is_empty() {
        match visited_country {
            VisitedCountryNaptrState::Failed(_) | VisitedCountryNaptrState::NotQueried => {}
            VisitedCountryNaptrState::Records(records) => {
                let usable = records
                    .iter()
                    .filter(|record| standard_epdg_record_plmn(record).is_some())
                    .collect::<Vec<_>>();
                if let Some(serving_plmn) = serving_plmn.as_deref() {
                    if usable
                        .iter()
                        .any(|record| standard_epdg_record_matches_plmn(record, serving_plmn))
                    {
                        // A serving VPLMN named by NAPTR is selected using its
                        // standard Operator Identifier form, not the replacement
                        // as an endpoint without first reconstructing the PLMN.
                        if let Some((mcc, mnc)) = canonical_plmn(serving_plmn)
                            .as_deref()
                            .map(|plmn| (&plmn[..3], &plmn[3..]))
                        {
                            if let Some(host) = profiles::standard_operator_epdg_fqdn(mcc, mnc) {
                                if let Some(address) = epdg_address_from_text(&host) {
                                    push_epdg_candidate(
                                        &mut candidates,
                                        address,
                                        EpdgCandidateSource::VisitedCountryNaptr,
                                    );
                                }
                            }
                        }
                    }
                }

                if candidates.is_empty() {
                    // If the response contains a PLMN covered by
                    // EFePDGSelection, select the highest-priority matching
                    // entry. DNS order is retained among records tied at that
                    // UICC priority.
                    let best_priority = uicc
                        .selection
                        .iter()
                        .filter(|entry| !entry.is_any_plmn())
                        .filter(|entry| {
                            usable.iter().any(|record| {
                                standard_epdg_record_plmn(record)
                                    .is_some_and(|plmn| entry.matches_plmn(&plmn))
                            })
                        })
                        .map(|entry| entry.priority)
                        .min();
                    if let Some(best_priority) = best_priority {
                        for entry in uicc
                            .selection
                            .iter()
                            .filter(|entry| !entry.is_any_plmn() && entry.priority == best_priority)
                        {
                            for record in &usable {
                                let Some(target_plmn) = standard_epdg_record_plmn(record) else {
                                    continue;
                                };
                                if entry.matches_plmn(&target_plmn) {
                                    if let Some(address) =
                                        selection_entry_address(entry, &target_plmn, location)
                                    {
                                        push_epdg_candidate(
                                            &mut candidates,
                                            address,
                                            EpdgCandidateSource::VisitedCountryNaptr,
                                        );
                                    }
                                }
                            }
                        }
                        // A LocationBased format for a PLMN other than the
                        // actual serving VPLMN cannot be constructed safely
                        // because the available TAC belongs to the serving
                        // cell. Do not invent a TAC, and do not bypass a
                        // matching UICC row with a raw DNS replacement.
                    } else {
                        // No UICC row covers the DNS response. This is the
                        // implementation-specific branch in TS 24.302; use
                        // only DNS-provided public Operator Identifier records
                        // in DNS order.
                        for record in usable {
                            if let Some(address) = epdg_address_from_text(&record.replacement) {
                                push_epdg_candidate(
                                    &mut candidates,
                                    address,
                                    EpdgCandidateSource::VisitedCountryNaptr,
                                );
                            }
                        }
                    }
                }
            }
            VisitedCountryNaptrState::EmptyResponse => {
                // An authoritative empty answer means that visited-country
                // selection is optional. Prefer the highest-priority concrete
                // UICC row in the visited country before the home/profile
                // fallback. Wildcard rows are omitted here because their MNC is
                // not known and a fabricated public FQDN would be unsafe.
                let serving_mcc = serving_plmn.as_deref().and_then(|plmn| plmn.get(..3));
                let best_priority = serving_mcc.and_then(|mcc| {
                    uicc.selection
                        .iter()
                        .filter(|entry| {
                            !entry.is_any_plmn() && selection_entry_is_in_country(entry, mcc)
                        })
                        .map(|entry| entry.priority)
                        .min()
                });
                if let Some(best_priority) = best_priority {
                    for entry in uicc.selection.iter().filter(|entry| {
                        !entry.is_any_plmn()
                            && entry.priority == best_priority
                            && serving_mcc
                                .is_some_and(|mcc| selection_entry_is_in_country(entry, mcc))
                    }) {
                        let Some(target_plmn) = concrete_selection_plmn(entry) else {
                            continue;
                        };
                        if let Some(address) =
                            selection_entry_address(entry, &target_plmn, location)
                        {
                            push_epdg_candidate(
                                &mut candidates,
                                address,
                                EpdgCandidateSource::UiccSelection,
                            );
                        }
                    }
                }
            }
        }
    }

    // UICC home identifiers and the profile-derived/configured host are the
    // final non-mandatory-country fallback path. In a private PLMN the profile
    // host must already be explicit; standard helpers refuse MCC 999.
    for address in &uicc.home_identifiers {
        push_epdg_candidate(
            &mut candidates,
            address.clone(),
            EpdgCandidateSource::UiccHomeIdentifier,
        );
    }

    if let Some(address) = epdg_address_from_text(profile.epdg.host) {
        push_epdg_candidate(
            &mut candidates,
            address,
            if profiles::is_standard_derived_profile(profile) {
                EpdgCandidateSource::HomePlmnDerived
            } else {
                EpdgCandidateSource::CarrierProfile
            },
        );
    }
    candidates
}

/// DNS servers to try, in order: this line's override first, then the carrier
/// profile's list. Resolving the ePDG FQDN is a hard prerequisite for
/// connecting at all, so a single unreachable resolver must not be fatal.
fn live_dns_candidates(line_id: &str, profile: &'static CarrierProfile) -> Vec<SocketAddr> {
    let mut candidates = Vec::new();
    for server in line_overrides(line_id).dns_servers {
        if !candidates.contains(&server) {
            candidates.push(server);
        }
    }
    for server in profile.epdg.dns_servers {
        if let Some(addr) = super::profile_record::parse_dns_server(server) {
            if !candidates.contains(&addr) {
                candidates.push(addr);
            }
        }
    }
    if let Some(server) = profile
        .epdg
        .dns_server
        .and_then(|value| super::profile_record::parse_dns_server(value))
    {
        if !candidates.contains(&server) {
            candidates.push(server);
        }
    }
    candidates
}

/// Build the resolver attempts in priority order. The final `None` attempt is
/// intentional: it delegates to the platform resolver after every explicit
/// line/profile DNS server has failed, so a stale custom DNS setting cannot
/// make ePDG discovery permanently fail when ordinary DNS still works.
fn live_dns_attempts(line_id: &str, profile: &'static CarrierProfile) -> Vec<Option<SocketAddr>> {
    let mut attempts = live_dns_candidates(line_id, profile)
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    attempts.push(None);
    attempts
}

fn standard_epdg_record_matches_plmn(record: &epdg::NaptrReplacement, plmn: &str) -> bool {
    let Some(record_plmn) = standard_epdg_record_plmn(record) else {
        return false;
    };
    super::qmi_uim::epdg_plmn_pattern_matches(plmn, &record_plmn)
}

async fn resolve_live_visited_country_naptr(
    line_id: &str,
    _profile: &'static CarrierProfile,
    uicc: &UsimEpdgConfig,
    location: Option<&EpdgLocationSnapshot>,
) -> VisitedCountryNaptrState {
    let Some(location) = location else {
        return VisitedCountryNaptrState::NotQueried;
    };
    let Some(serving_plmn) = canonical_plmn(&location.serving_plmn) else {
        return VisitedCountryNaptrState::NotQueried;
    };
    let serving_mcc = &serving_plmn[..3];
    let Some(serving_country_fqdn) = profiles::standard_visited_country_epdg_fqdn(serving_mcc)
    else {
        // This also rejects MCC 999, so a private serving PLMN never causes a
        // public visited-country DNS name to be guessed.
        return VisitedCountryNaptrState::NotQueried;
    };
    let roaming = serving_is_roaming(Some(location), _profile.meta.plmn);
    if !roaming {
        return VisitedCountryNaptrState::NotQueried;
    }
    let has_selected_plmn = uicc
        .selection
        .iter()
        .any(|entry| entry.is_any_plmn() || entry.matches_plmn(&serving_plmn));
    if has_selected_plmn {
        // An explicit VPLMN/Any_PLMN UICC row has precedence over visited-
        // country discovery and therefore suppresses the NAPTR query.
        return VisitedCountryNaptrState::NotQueried;
    }

    let proxy = line_overrides(line_id).proxy;
    let mut last_error = None;
    for dns_server in live_dns_attempts(line_id, _profile) {
        let result = match &proxy {
            Some(LiveProxySetting::Socks5(endpoint)) => {
                epdg::resolve_visited_country_naptr_via_socks5(
                    &serving_country_fqdn,
                    dns_server,
                    endpoint,
                )
                .await
            }
            None => {
                epdg::resolve_visited_country_naptr_with_dns_override(
                    &serving_country_fqdn,
                    dns_server,
                )
                .await
            }
        };
        match result {
            Ok(records) if records.is_empty() => {
                return VisitedCountryNaptrState::EmptyResponse;
            }
            Ok(records) => return VisitedCountryNaptrState::Records(records),
            Err(error) => {
                warn!(
                    line_id = %line_id,
                    dns_server = ?dns_server,
                    fqdn = %serving_country_fqdn,
                    error = %error,
                    "visited-country NAPTR lookup failed; trying the next DNS candidate"
                );
                last_error = Some(error);
            }
        }
    }

    let error = last_error
        .map(|error| error.to_string())
        .unwrap_or_else(|| "no_dns_candidate_available".to_string());
    warn!(
        line_id = %line_id,
        fqdn = %serving_country_fqdn,
        error = %error,
        "visited-country NAPTR discovery received no DNS response; terminating ePDG selection"
    );
    VisitedCountryNaptrState::Failed(error)
}

async fn resolve_live_epdg_candidate(
    line_id: &str,
    profile: &'static CarrierProfile,
    candidate: &EpdgEndpointCandidate,
) -> Result<ResolvedEpdgEndpoint, TransportError> {
    let overrides = line_overrides(line_id);
    let host = candidate.host();
    let port = overrides.epdg_port.unwrap_or(profile.epdg.port);
    // DNS follows the proxy: when this line egresses through a proxy, the lookup
    // goes through it too, so the real client IP is not exposed to the resolver
    // and operator DNS interception on the ePDG name is bypassed. With no proxy
    // configured the query goes out directly to the configured server.
    let proxy = overrides.proxy;
    // Always try the platform resolver last, even when explicit DNS servers
    // were configured. This preserves the documented custom -> profile ->
    // system fallback chain.
    if let UsimEpdgAddress::Ip(ip) = &candidate.address {
        let route_policy = choose_route_policy(
            &profile.meta,
            &host,
            proxy.as_ref().map(|_| ProxyKind::Socks5UdpAssociate),
        );
        return Ok(ResolvedEpdgEndpoint {
            host,
            port,
            addresses: vec![SocketAddr::new(*ip, port)],
            route_policy,
        });
    }
    let dns_attempts = live_dns_attempts(line_id, profile);
    let mut last_error = None;
    for dns_server in dns_attempts {
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

async fn resolve_live_epdg_candidates(
    line_id: &str,
    profile: &'static CarrierProfile,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<Vec<ResolvedEpdgEndpoint>, TransportError> {
    let uicc = live_uicc_epdg_config(line_id).await.unwrap_or_default();
    let location = access_network.epdg_location();
    let visited_country_records =
        resolve_live_visited_country_naptr(line_id, profile, &uicc, location.as_ref()).await;
    let candidates = build_live_epdg_candidates_with_naptr(
        line_id,
        profile,
        &uicc,
        location.as_ref(),
        &visited_country_records,
    );
    if candidates.is_empty() {
        return Err(TransportError::DnsFailed(
            "no_valid_epdg_host_candidate".to_string(),
        ));
    }

    let mut resolved = Vec::new();
    let mut last_error = None;
    for candidate in candidates {
        let host = candidate.host();
        let attempt = tokio::time::timeout(
            LIVE_DNS_TIMEOUT,
            resolve_live_epdg_candidate(line_id, profile, &candidate),
        )
        .await;
        match attempt {
            Ok(Ok(endpoint)) => resolved.push(endpoint),
            Ok(Err(error)) => {
                warn!(
                    line_id = %line_id,
                    candidate_source = candidate.source.as_str(),
                    host = %host,
                    error = %error,
                    "ePDG candidate failed; trying the next standards/profile candidate"
                );
                last_error = Some(error);
            }
            Err(_) => {
                warn!(
                    line_id = %line_id,
                    candidate_source = candidate.source.as_str(),
                    host = %host,
                    "ePDG candidate resolution timed out; trying the next candidate"
                );
                last_error = Some(TransportError::Timeout(
                    "epdg_candidate_resolution_timeout".to_string(),
                ));
            }
        }
    }
    if resolved.is_empty() {
        Err(last_error
            .unwrap_or_else(|| TransportError::DnsFailed("no_epdg_candidate_resolved".to_string())))
    } else {
        Ok(resolved)
    }
}

async fn resolve_live_epdg(
    line_id: &str,
    profile: &'static CarrierProfile,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<ResolvedEpdgEndpoint, TransportError> {
    resolve_live_epdg_candidates(line_id, profile, access_network)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| TransportError::DnsFailed("no_epdg_candidate_resolved".to_string()))
}

#[derive(Debug, Clone)]
struct LiveImsRegisterReady {
    profile_id: &'static str,
    expires_at: Instant,
    registration: RegisteredImsContext,
    sms_capability_advertised: bool,
    receiver_transport: &'static str,
}

#[derive(Debug, Clone)]
struct LiveImsSecurityVerify {
    profile_id: &'static str,
    expires_at: Instant,
    value: String,
}

struct LiveImsChannel {
    profile_id: &'static str,
    expires_at: Instant,
    channel: SipChannel,
}

#[derive(Clone)]
struct LiveXcapBinding {
    profile: &'static CarrierProfile,
    local_address: IpAddr,
    username: String,
}

struct VowifiXcapDigestProvider {
    line_id: String,
    username: String,
}

impl XcapDigestProvider for VowifiXcapDigestProvider {
    fn authorize<'a>(
        &'a self,
        challenge: &'a str,
        proxy: bool,
        method: &'a str,
        uri: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<String, crate::connectivity::core::ut::UtError>>
    {
        Box::pin(async move {
            build_line_digest_aka_authorization(
                &self.line_id,
                &self.username,
                method,
                uri,
                challenge,
                proxy,
            )
            .await
            .map_err(|_| crate::connectivity::core::ut::UtError::new("ut_xcap_aka_failed"))
        })
    }
}

#[derive(Debug, Clone)]
struct LiveImsRegisterSuccessVariant {
    profile_id: &'static str,
    /// Runtime address of the exact immutable profile used for the successful
    /// exchange. Custom profile reloads leak a new immutable value, so the
    /// address prevents a cached request shape from crossing profile edits that
    /// happen to reuse the same profile ID.
    profile_address: usize,
    /// Cache the full response-driven shape rather than only its label. Dynamic
    /// variants do not exist in the static candidate table and otherwise cannot
    /// be preferred during refresh or a reconnect.
    variant: LiveRegisterHeaderVariant,
    captured_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveImsRefreshFailure {
    consecutive_failures: u8,
    last_failure_reason: String,
    rebuild_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveImsRefreshFailureDecision {
    Retry,
    RebuildAccess,
    RebuildPending,
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
fn tun_name_for_line(base: &str, line_id: &str) -> String {
    // `line_id` is `line-<md5>`; 32 bits of its hex digest keeps collision risk
    // negligible for multi-line hosts while still fitting inside IFNAMSIZ.
    let suffix: String = line_id
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .rev()
        .take(8)
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
/// Every line has to run SIM authentication and IKE identity against *its own*
/// reader, otherwise line B would authenticate with line A's card. Registering
/// each discovered line binding here keeps that mapping explicit and leaves no
/// process-global QMI device fallback.
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
    pub pcsc_reader: String,
}

fn line_sim_devices() -> &'static StdRwLock<HashMap<String, LiveSimDevice>> {
    LIVE_LINE_SIM_DEVICES.get_or_init(|| StdRwLock::new(HashMap::new()))
}

/// Everything the VoWiFi data plane needs from a line's UE worker: the
/// namespace, the UE-side veth name (IKE egress) and the worker handle that
/// creates sockets inside the namespace.
#[derive(Clone)]
pub(crate) struct LiveUeSocketContext {
    pub namespace: String,
    pub ue_veth: String,
    pub worker: UeWorkerHandle,
}

static LIVE_UE_SOCKET_CONTEXTS: OnceLock<StdRwLock<HashMap<String, LiveUeSocketContext>>> =
    OnceLock::new();

fn line_ue_socket_contexts() -> &'static StdRwLock<HashMap<String, LiveUeSocketContext>> {
    LIVE_UE_SOCKET_CONTEXTS.get_or_init(|| StdRwLock::new(HashMap::new()))
}

/// Record (or clear) the UE socket context for a line. Called during every
/// line refresh so a config toggle applies on the next VoWiFi reconnect.
pub(crate) fn register_line_ue_socket_context(line_id: &str, context: Option<LiveUeSocketContext>) {
    let mut guard = line_ue_socket_contexts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match context {
        Some(context) => {
            guard.insert(line_id.to_string(), context);
        }
        None => {
            guard.remove(line_id);
        }
    }
}

/// Resolve the UE socket context for a line. `None` keeps the current
/// host-namespace socket creation path.
pub(crate) fn ue_socket_context_for_line(line_id: &str) -> Option<LiveUeSocketContext> {
    line_ue_socket_contexts()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned()
}

/// Resolve the UE namespace this line's VoWiFi tunnel should live in. The
/// namespace and socket owner deliberately come from the same registry, so a
/// refresh can never place the TUN on the host while IKE/SIP use the worker.
pub(crate) fn ue_namespace_for_line(line_id: &str) -> Option<String> {
    ue_socket_context_for_line(line_id).map(|context| context.namespace)
}

/// Return the common per-UE operator socket factory for another IMS access
/// leg.  VoLTE uses this for RTP/RTCP/video while VoWiFi uses the same factory
/// internally; keeping construction here guarantees both legs resolve the
/// same line-owned worker.
pub(crate) fn operator_socket_creator_for_line(
    line_id: &str,
) -> Option<Arc<dyn OperatorSocketCreator>> {
    ue_socket_context_for_line(line_id).map(|context| {
        Arc::new(super::operator::UeWorkerOperatorSocketCreator::new(
            context.worker,
        )) as Arc<dyn OperatorSocketCreator>
    })
}

fn publish_line_sim_device(line_id: &str, device: LiveSimDevice) {
    let changed = {
        let mut devices = line_sim_devices()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if devices.get(line_id) == Some(&device) {
            false
        } else {
            devices.insert(line_id.to_string(), device);
            true
        }
    };
    // Discovery refreshes can repeat the same descriptor every few seconds.
    // Keep the UICC optional-file cache in that common case; invalidate it only
    // when the actual reader/slot/modem binding changes.
    if changed {
        forget_live_uicc_epdg_config(line_id);
    }
}

/// Record which reader a line owns. Called when lines are discovered/refreshed.
pub fn register_line_sim_device(line_id: &str, qmi_device: &str, uim_slot: u8, modem_path: &str) {
    if line_id.is_empty() || (qmi_device.is_empty() && modem_path.is_empty()) {
        return;
    }
    publish_line_sim_device(
        line_id,
        LiveSimDevice {
            qmi_device: qmi_device.to_string(),
            uim_slot,
            modem_path: modem_path.to_string(),
            pcsc_reader: String::new(),
        },
    );
}

/// Record a standalone PC/SC reader owned by one line.
pub fn register_line_pcsc_reader(line_id: &str, reader_path: &str) {
    if line_id.trim().is_empty()
        || crate::hardware::devices::pcsc::selector_from_path(reader_path).is_none()
    {
        return;
    }
    publish_line_sim_device(
        line_id,
        LiveSimDevice {
            qmi_device: String::new(),
            uim_slot: 0,
            modem_path: String::new(),
            pcsc_reader: reader_path.trim().to_string(),
        },
    );
}

/// Resolve this line's SIM identity (IMSI etc.) through ModemManager.
///
/// Uses the line's own `modem_path` so a second line reports its own subscriber
/// rather than the first modem's. Missing mappings fail closed.
async fn line_sim_identity(
    line_id: &str,
    conn: &zbus::Connection,
) -> Option<crate::hardware::cellular::modem_manager::SimIdentity> {
    let device = sim_device_for_line(line_id);
    if !device.pcsc_reader.is_empty() {
        return tokio::task::spawn_blocking(move || {
            crate::hardware::devices::pcsc::read_identity(&device.pcsc_reader)
                .ok()
                .map(|identity| {
                    let operator_id = identity
                        .mnc_length
                        .filter(|length| identity.imsi.len() >= 3 + *length as usize)
                        .map(|length| identity.imsi[..3 + length as usize].to_string())
                        .unwrap_or_default();
                    crate::hardware::cellular::modem_manager::SimIdentity {
                        iccid: identity.iccid,
                        imsi: identity.imsi,
                        operator_id,
                    }
                })
        })
        .await
        .ok()
        .flatten();
    }
    if !device.modem_path.is_empty() {
        if let Some(identity) = crate::hardware::cellular::modem_manager::sim_identity_for_modem(
            conn,
            &device.modem_path,
        )
        .await
        {
            return Some(identity);
        }
    }
    None
}

async fn line_sim_info(
    line_id: &str,
    conn: &zbus::Connection,
) -> Option<crate::api::models::SimInfoResponse> {
    let device = sim_device_for_line(line_id);
    if !device.pcsc_reader.is_empty() {
        return tokio::task::spawn_blocking(move || {
            crate::hardware::devices::pcsc::read_identity(&device.pcsc_reader)
                .ok()
                .map(|identity| {
                    let mnc_length = identity.mnc_length.unwrap_or(0) as usize;
                    let (mcc, mnc) = if mnc_length > 0 && identity.imsi.len() >= 3 + mnc_length {
                        (
                            identity.imsi[..3].to_string(),
                            identity.imsi[3..3 + mnc_length].to_string(),
                        )
                    } else {
                        (String::new(), String::new())
                    };
                    crate::api::models::SimInfoResponse {
                        present: true,
                        active: true,
                        iccid: identity.iccid,
                        imsi: identity.imsi,
                        mcc,
                        mnc,
                        sim_path: device.pcsc_reader,
                        sim_type: "physical".to_string(),
                        lock_status: "none".to_string(),
                        ..Default::default()
                    }
                })
        })
        .await
        .ok()
        .flatten();
    }
    if !device.modem_path.is_empty() {
        if let Ok(info) = get_sim_info_for_modem_with_cache(conn, &device.modem_path, None).await {
            return Some(info);
        }
    }
    None
}

/// Forget only a line's reader mapping during discovery refresh.
///
/// UE namespace/socket ownership is intentionally left intact here. The line
/// registry refreshes reader bindings before it reconciles the existing UE;
/// clearing the network registries at that point can leave the egress
/// fingerprint unchanged and prevent them from being published again.
pub fn forget_line_sim_device_mapping(line_id: &str) {
    line_sim_devices()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(line_id);
    forget_live_uicc_epdg_config(line_id);
}

/// Forget all live state for a line (line removed or its UE torn down).
pub fn forget_line_sim_device(line_id: &str) {
    forget_line_sim_device_mapping(line_id);
    line_ue_socket_contexts()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(line_id);
}

/// Resolve the SIM device for a line. Missing and unknown lines return an empty
/// device so they can never authenticate with another line's reader.
pub(crate) fn sim_device_for_line(line_id: &str) -> LiveSimDevice {
    if let Some(device) = line_sim_devices()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned()
    {
        return device;
    }
    LiveSimDevice {
        qmi_device: String::new(),
        uim_slot: 0,
        modem_path: String::new(),
        pcsc_reader: String::new(),
    }
}

/// Read optional TS 31.102 ePDG information from this line's exact UICC.
///
/// The cache is bound to the full reader descriptor, not merely the line id, so
/// replacing a card/reader cannot reuse the former line's ePDG identifiers.
/// Optional-file failures are deliberately non-fatal: user/catalog/profile
/// candidates remain available and the failed read is cached briefly to avoid
/// hammering a card that does not implement 6FF3/6FF4.
async fn live_uicc_epdg_config(line_id: &str) -> Result<UsimEpdgConfig, &'static str> {
    let device = sim_device_for_line(line_id);
    if device.pcsc_reader.is_empty() && device.qmi_device.is_empty() {
        return Ok(UsimEpdgConfig::default());
    }

    if let Some(cached) = live_uicc_epdg_config_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .filter(|cached| {
            cached.device == device && cached.loaded_at.elapsed() <= LIVE_UICC_EPDG_CACHE_TTL
        })
        .cloned()
    {
        return Ok(cached.config);
    }

    let proxy_socket = live_runtime_config().qmi_proxy_socket;
    let read_device = device.clone();
    let result = tokio::task::spawn_blocking(move || {
        if !read_device.pcsc_reader.is_empty() {
            crate::hardware::devices::pcsc::read_epdg_config(&read_device.pcsc_reader)
        } else {
            read_usim_epdg_config_via_proxy_reason(
                &proxy_socket,
                &read_device.qmi_device,
                read_device.uim_slot,
                USIM_AID_PREFIX,
                LIVE_SIM_AUTH_TIMEOUT,
            )
        }
    })
    .await
    .map_err(|_| "sim_epdg_config_worker_failed")?;

    let config = match result {
        Ok(config) => config,
        Err(reason) => {
            warn!(
                line_id = %line_id,
                reason,
                "Optional UICC ePDG configuration could not be read; continuing with configured and standard candidates"
            );
            UsimEpdgConfig::default()
        }
    };
    live_uicc_epdg_config_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            line_id.to_string(),
            CachedLiveUiccEpdgConfig {
                device,
                loaded_at: Instant::now(),
                config: config.clone(),
            },
        );
    Ok(config)
}

/// Verify SIM auth access for a specific line's reader.
pub async fn verify_live_sim_auth_access_for_line(line_id: &str) -> Result<(), LiveStageError> {
    let proxy_socket = live_runtime_config().qmi_proxy_socket;
    let device = sim_device_for_line(line_id);
    tokio::task::spawn_blocking(move || {
        if !device.pcsc_reader.is_empty() {
            crate::hardware::devices::pcsc::verify_usim(&device.pcsc_reader)
        } else if device.qmi_device.is_empty() {
            Err("sim_auth_device_unavailable")
        } else {
            verify_usim_application_via_proxy_reason_with_retry(
                proxy_socket.as_str(),
                device.qmi_device.as_str(),
                device.uim_slot,
                USIM_AID_PREFIX,
                LIVE_SIM_AUTH_GATE_ATTEMPTS,
                LIVE_SIM_AUTH_GATE_TIMEOUT,
                LIVE_SIM_AUTH_GATE_RETRY_DELAY,
            )
        }
    })
    .await
    .map_err(|_| live_stage_error("sim_auth_gate_runtime_failed"))?
    .map_err(live_stage_error)?;
    info!("SIMAuth access gate passed");
    Ok(())
}

/// Authenticate against the exact SIM reader registered for `line_id`.
pub async fn authenticate_live_sim_for_line(
    line_id: &str,
    rand: &[u8],
    autn: &[u8],
) -> Result<super::qmi_uim::UsimAkaApduResult, &'static str> {
    let proxy_socket = live_runtime_config().qmi_proxy_socket;
    let device = sim_device_for_line(line_id);
    let rand = rand.to_vec();
    let autn = autn.to_vec();
    tokio::task::spawn_blocking(move || {
        if !device.pcsc_reader.is_empty() {
            crate::hardware::devices::pcsc::authenticate(&device.pcsc_reader, &rand, &autn)
        } else if device.qmi_device.is_empty() {
            Err("sim_auth_device_unavailable")
        } else {
            execute_usim_authenticate_via_proxy_reason_with_retry(
                proxy_socket.as_str(),
                device.qmi_device.as_str(),
                device.uim_slot,
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
    .map_err(|_| "sim_auth_runtime_failed")?
}

fn read_non_empty_config(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
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
    /// True only after the registrar explicitly demanded sec-agree. This is
    /// separate from a profile-level `required` policy so dynamic retries can
    /// preserve the network-selected declaration without overriding an
    /// explicitly disabled profile.
    server_required_sec_agree: bool,
    /// Challenge-first fallback: keep sec-agree headers off the initial
    /// REGISTER even when the profile marks security_agreement as required.
    suppress_sec_agree_headers: bool,
    include_route_header: bool,
    include_security_client: bool,
    initial_authorization: LiveInitialAuthorizationFormat,
    security_client_format: LiveSecurityClientFormat,
    request_uri: LiveRegisterRequestUri,
    identity_format: LiveRegisterIdentityFormat,
    header_profile: LiveRegisterHeaderProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    HomeDomain,
    HomeRegistrar,
    PcscfSocket,
}

impl LiveRegisterRequestUri {
    fn label(self) -> &'static str {
        match self {
            Self::HomeDomain => "home_domain",
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
    /// SIMADMIN_COMPACT_REGISTER=1 trims optional REGISTER headers
    /// (Cellular-Network-Info, Contact feature tags, +sip.instance/reg-id)
    /// so the whole tunnel packet fits in the path MTU without IP
    /// fragmentation. Real VoWiFi handsets keep REGISTERs small for the
    /// same reason.
    compact_register: bool,
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
        compact_register: false,
    };

    const IMS_FEATURES: Self = Self {
        contact_features: LiveContactFeatureSet::MmtelSmsSipInstance,
        include_accept_contact: true,
        include_p_preferred_identity: true,
        visited_network: LiveVisitedNetworkFormat::QuotedHome,
        pani: LivePaniFormat::ProfileDefault,
        include_cellular_network_info: true,
        user_agent: LiveUserAgentFormat::ProfileDefault,
        compact_register: false,
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

#[cfg(test)]
const LIVE_REGISTER_HEADER_VARIANTS: &[LiveRegisterHeaderVariant] = &[
    LiveRegisterHeaderVariant {
        label: "ims_features_aka_uri_first_full_sec_client",
        force_sec_agree_headers: false,
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
        include_route_header: true,
        include_security_client: true,
        initial_authorization: LiveInitialAuthorizationFormat::None,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: LiveRegisterRequestUri::HomeRegistrar,
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile: LiveRegisterHeaderProfile::IMS_FEATURES,
    },
];

#[cfg(test)]
const GB_EE_REGISTER_HEADER_VARIANTS: &[LiveRegisterHeaderVariant] = &[
    LiveRegisterHeaderVariant {
        label: "gb_ee_aka_uri_first_sec_client",
        force_sec_agree_headers: false,
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
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
    registration_loss: Option<RegistrationLossReason>,
    /// The core answered 421 naming `sec-agree`. Bundles captured from real
    /// handsets often leave `security_agreement` unset, which resolves to
    /// "auto" and emits a Security-Client offer with no Require/Proxy-Require.
    /// Recording the demand here lets the variant loop satisfy it instead of
    /// retrying the same rejected shape.
    server_required_sec_agree: bool,
    /// Authentication rounds completed before the shared REGISTER engine gave
    /// up. Dynamic request-shape changes are forbidden once AKA has started.
    register_auth_rounds: u8,
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
    access_network: ImsAccessNetworkRuntime,
}

impl SystemLiveEpdgAdapter {
    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self::for_line_with_access_network(line_id, ImsAccessNetworkRuntime::default())
    }

    pub fn for_line_with_access_network(
        line_id: impl Into<String>,
        access_network: ImsAccessNetworkRuntime,
    ) -> Self {
        Self {
            line_id: line_id.into(),
            access_network,
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
            resolve_live_epdg(&self.line_id, profile, &self.access_network)
                .await
                .map_err(|_| live_stage_error("epdg_dns_resolution_failed"))
        })
    }
}

/// Runs the live UDP-path stages for one line, carrying that line's ePDG/DNS/proxy
/// overrides so concurrent lines stay independent.
#[derive(Debug, Clone, Default)]
pub struct SystemLiveDatagramAdapter {
    line_id: String,
    access_network: ImsAccessNetworkRuntime,
}

impl SystemLiveDatagramAdapter {
    pub fn for_line(line_id: impl Into<String>) -> Self {
        Self::for_line_with_access_network(line_id, ImsAccessNetworkRuntime::default())
    }

    pub fn for_line_with_access_network(
        line_id: impl Into<String>,
        access_network: ImsAccessNetworkRuntime,
    ) -> Self {
        Self {
            line_id: line_id.into(),
            access_network,
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
                ExecutorStage::Ike => run_live_ike_until(
                    &self.line_id,
                    profile,
                    LiveIkeTarget::EapSuccess,
                    &self.access_network,
                )
                .await
                .map(|_| ()),
                ExecutorStage::ChildSa | ExecutorStage::Esp => {
                    run_live_esp_until(&self.line_id, profile, &self.access_network).await
                }
                ExecutorStage::ImsRegister => {
                    run_live_ims_register_until(&self.line_id, profile, &self.access_network).await
                }
                ExecutorStage::Sms => {
                    run_live_sms_until(&self.line_id, profile, &self.access_network).await
                }
                ExecutorStage::Voice => {
                    run_live_voice_until(&self.line_id, profile, &self.access_network).await
                }
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveIkeSession, LiveStageError> {
    run_live_ike_until_depth(
        line_id,
        profile,
        target,
        LiveProbeDepth::FullHandshake,
        access_network,
    )
    .await
}

async fn run_live_ike_until_depth(
    line_id: &str,
    profile: &'static CarrierProfile,
    target: LiveIkeTarget,
    depth: LiveProbeDepth,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveIkeSession, LiveStageError> {
    let endpoints = resolve_live_epdg_candidates(line_id, profile, access_network)
        .await
        .map_err(map_transport_error)?;
    if endpoints.is_empty() {
        error!("No ePDG endpoint candidates resolved");
        return Err(live_stage_error("epdg_no_address"));
    }

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
    let proposal_groups = proposal_groups
        .iter()
        .take(proposal_group_limit)
        .collect::<Vec<_>>();

    let mut last_error = None;
    // Preserve standards/profile priority all the way through IKE. DNS success
    // alone does not prove that an ePDG accepts this SIM, so exhaust the selected
    // host's addresses/proposals/transport paths before moving to the next UICC
    // or profile-derived host.
    for endpoint in endpoints {
        let selected_epdg_host = endpoint.host;
        let addresses = endpoint
            .addresses
            .into_iter()
            .take(endpoint_limit)
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            warn!(host = %selected_epdg_host, "Resolved ePDG candidate had no addresses");
            continue;
        }
        info!(
            host = %selected_epdg_host,
            addresses = ?addresses,
            "Trying resolved ePDG candidate"
        );
        for address in addresses {
            for proposal_group in &proposal_groups {
                for path in transport_paths {
                    let mut destination = address;
                    destination.set_port(path.destination_port);
                    info!(
                        host = %selected_epdg_host,
                        destination = ?destination,
                        local_port_preferred = path.preferred_local_port,
                        initial_nat_t = path.initial_nat_t,
                        "Attempting IKE connection path"
                    );
                    match run_live_ike_with_destination(
                        line_id,
                        profile,
                        target,
                        &selected_epdg_host,
                        destination,
                        *path,
                        proposal_group,
                    )
                    .await
                    {
                        Ok(session) => {
                            info!(
                                host = %selected_epdg_host,
                                selected_ike_proposals = ?proposal_group.proposals,
                                destination = ?destination,
                                "Successfully established IKE session"
                            );
                            return Ok(session);
                        }
                        Err(error) => {
                            warn!(
                                host = %selected_epdg_host,
                                destination = ?destination,
                                local_port_preferred = path.preferred_local_port,
                                error = ?error,
                                "IKE connection path failed; continuing fallback ladder"
                            );
                            last_error = Some(error);
                        }
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
    selected_epdg_host: &str,
    destination: SocketAddr,
    path: LiveIkeTransportPath,
    proposal_group: &LiveIkeProposalGroup,
) -> Result<LiveIkeSession, LiveStageError> {
    let ue_socket = ue_socket_context_for_line(line_id);
    let local_addr = match &ue_socket {
        Some(_) => {
            let base = unspecified_local_addr_for(destination);
            SocketAddr::new(base.ip(), path.preferred_local_port)
        }
        None => local_bind_addr_for_destination(destination, path.preferred_local_port)
            .await
            .unwrap_or_else(|_| unspecified_local_addr_for(destination)),
    };
    info!(
        "run_live_ike_with_destination: binding local_addr={:?} for destination={:?}",
        local_addr, destination
    );
    let transport = match &ue_socket {
        Some(context) => {
            // Use udp_bound (not udp_connected) because IKE switches from
            // port 500 to port 4500 during NAT-T. A connect()ed UDP socket
            // on Linux rejects sendto() to a different peer and recvfrom()
            // only accepts from the connected peer - both break NAT-T.
            let spec = UeSocketSpec::udp_bound(
                local_addr,
                Some(context.ue_veth.clone()),
            );
            match context.worker.create_socket(spec).await {
                Ok(UeSocket::Udp(socket)) => {
                    info!(
                        line_id,
                        ue_veth = %context.ue_veth,
                        "IKE transport socket created inside UE namespace"
                    );
                    UdpSocketDatagramTransport::from_socket(socket)
                }
                Ok(_) => return Err(live_stage_error("ike_ue_socket_family_mismatch")),
                Err(error) => {
                    warn!(
                        line_id,
                        error = %error,
                        "UE worker IKE socket creation failed; VoWiFi path aborted for this destination"
                    );
                    return Err(live_stage_error("ike_ue_socket_creation_failed"));
                }
            }
        }
        None => UdpSocketDatagramTransport::bind(local_addr)
            .await
            .map_err(map_transport_error)?,
    }
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
    let mut machine = IkeStateMachine::new_with_dh_group_and_access(
        profile,
        live_ike_access_for_epdg(line_id, profile, Some(selected_epdg_host)),
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
            live_stage_error(reason)
        })?;
    info!("Decrypting EAP-AKA challenge...");
    let eap_challenge = machine
        .decrypted_eap_aka_challenge_packet(&auth_response)
        .map_err(|_| live_stage_error("ike_auth_eap_challenge_decode_failed"))?;
    let challenge = parse_challenge(&eap_challenge)
        .map_err(|_| live_stage_error("eap_aka_challenge_parse_failed"))?;
    info!("Spawning USIM Authentication for the selected line reader...");
    // Authenticate against THIS line's reader. Using the global device would make
    // a second line run EAP-AKA against the first line's card, which fails
    // authentication (or worse, succeeds with the wrong subscriber identity).
    let aka_result = authenticate_live_sim_for_line(line_id, &challenge.rand, &challenge.autn)
        .await
        .map_err(|reason| {
            error!("USIM Authentication failed: {}", reason);
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
                live_stage_error(reason)
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
            .map_err(live_stage_error)?;
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<(), LiveStageError> {
    if cached_tun_gateway_matches(line_id, profile).await {
        return Ok(());
    }

    info!("Live ESP stage check: building full ePDG IKE/EAP-AKA/CHILD_SA path...");
    let session = run_live_ike_until(
        line_id,
        profile,
        LiveIkeTarget::ChildSaReady,
        access_network,
    )
    .await?;
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
    let inner_addr = select_inner_address(line_id, profile, configuration)
        .ok_or_else(|| live_stage_error("live_inner_address_missing"))?;
    let pcscf_addr = select_pcscf_address(line_id, profile, configuration, inner_addr)
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
        pcscf_addrs: pcscf_candidates(line_id, profile, configuration, inner_addr),
        inbound_sa_identifier: child_sa.inbound_sa_identifier,
        outbound_sa_identifier: child_sa.outbound_sa_identifier,
        secrets: child_sa.secrets.clone(),
        transport,
        remote,
        ue_namespace: ue_namespace_for_line(line_id),
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
    line_id: &str,
    profile: &'static CarrierProfile,
    configuration: &IkeConfigurationMaterial,
) -> Option<IpAddr> {
    if live_ike_access(line_id, profile).ip_stack.contains("ipv6") {
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
    line_id: &str,
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

    live_ims_target(line_id, profile)
        .pcscf
        .into_iter()
        .find_map(|pcscf| pcscf.parse::<IpAddr>().ok())
        .filter(|addr| addr.is_ipv4() == inner_addr.is_ipv4())
}

fn pcscf_candidates(
    line_id: &str,
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
    for static_addr in live_ims_target(line_id, profile)
        .pcscf
        .into_iter()
        .filter_map(|pcscf| pcscf.parse::<IpAddr>().ok())
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<(), LiveStageError> {
    let attempt = attempt_live_ims_registration(line_id, profile, access_network).await;
    let (outcome, error) = match attempt {
        Ok(registered) => (RegistrationRefreshResult::Refreshed(registered), None),
        Err(error) => {
            let loss_reason = error
                .registration_loss
                .unwrap_or(RegistrationLossReason::SignalingTransportLost);
            (
                RegistrationRefreshResult::RebuildAccess(loss_reason),
                Some(error),
            )
        }
    };

    match outcome {
        RegistrationRefreshResult::Refreshed(registered) => {
            record_live_ims_register_ready(line_id, profile, true, registered).await;
            Ok(())
        }
        RegistrationRefreshResult::RebuildAccess(loss_reason) => {
            warn!(
                line_id,
                profile_id = profile.meta.profile_id,
                registration_loss = loss_reason.as_str(),
                "VoWiFi IMS registration attempt requires access rebuild"
            );
            Err(error.expect("failed registration retains its adapter error"))
        }
    }
}

fn log_vowifi_register_binding_diagnostics(artifacts: &RegisterArtifacts) {
    if artifacts.contact_binding_count > 1 {
        warn!(
            contact_binding_count = artifacts.contact_binding_count,
            "Registrar returned multiple current Contact bindings; terminating routing may select another flow"
        );
    }
    if artifacts.contact_expiry_ambiguous {
        warn!(
            contact_binding_count = artifacts.contact_binding_count,
            "Contact expiry differs or is missing across registrar bindings; using response Expires or the profile fallback"
        );
    }
    if artifacts.wildcard_contact_present {
        warn!("Successful REGISTER response unexpectedly included a wildcard Contact");
    }
}

async fn attempt_live_ims_registration(
    line_id: &str,
    profile: &'static CarrierProfile,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<RegisteredImsContext, LiveStageError> {
    info!("Live ImsRegister stage check: verifying outer ESP tunnel and IMS TCP path...");
    run_live_esp_until(line_id, profile, access_network)
        .await
        .map_err(|error| {
            error.with_registration_loss(RegistrationLossReason::AccessTransportLost)
        })?;
    let gateway = cached_tun_gateway(line_id, profile)
        .await
        .map_err(|error| {
            error.with_registration_loss(RegistrationLossReason::AccessTransportLost)
        })?;
    let response =
        run_register_exchange_over_tunnel(line_id, profile, &gateway, access_network).await?;
    let parsed = ims::parse_sip_response(&response, &live_ims_target(line_id, profile).realm)
        .map_err(|_| {
            live_registration_error(
                "ims_register_response_parse_failed",
                RegistrationLossReason::NetworkRejected,
            )
        })?;
    let artifacts = RegisterArtifacts::parse(response.as_bytes());
    let service_route_count = artifacts.service_route_count;
    let contact_binding_count = artifacts.contact_binding_count;
    let contact_expiry_ambiguous = artifacts.contact_expiry_ambiguous;
    let wildcard_contact_present = artifacts.wildcard_contact_present;
    if parsed.status_code == 200 {
        log_vowifi_register_binding_diagnostics(&artifacts);
    }
    let registered = RegisteredImsContext::from_artifacts(
        ImsRegistrationAccess::Vowifi,
        artifacts,
        profile.ims.register.expires_seconds,
    );
    info!(
        status_code = parsed.status_code,
        reason = parsed.reason.as_str(),
        service_route_present = registered.service_route.is_some(),
        service_route_count,
        associated_uri_count = registered.associated_uris.len(),
        contact_binding_count,
        contact_expiry_ambiguous,
        wildcard_contact_present,
        warning_present = parsed.warning_present,
        unsupported = ?parsed.unsupported,
        require = ?parsed.require,
        proxy_require = ?parsed.proxy_require,
        "IMS REGISTER final response metadata received"
    );

    match parsed.status_code {
        200 => Ok(registered),
        401 | 403 | 407 => Err(live_registration_error(
            "ims_register_auth_rejected",
            RegistrationLossReason::AuthenticationRejected,
        )),
        _ => Err(live_registration_error(
            "ims_register_unexpected_status",
            RegistrationLossReason::NetworkRejected,
        )),
    }
}

async fn run_live_sms_until(
    line_id: &str,
    profile: &'static CarrierProfile,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<(), LiveStageError> {
    info!("Live Sms stage check: verifying protected IMS registration and SMSIP readiness...");
    match profile.sms.receiver_transport {
        "tcp" | "udp" => {}
        _ => return Err(live_stage_error("sms_receiver_transport_unsupported")),
    }

    if !cached_live_ims_register_ready(line_id, profile).await {
        run_live_ims_register_until(line_id, profile, access_network).await?;
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<(), LiveStageError> {
    info!("Live Voice stage check: verifying IMS registration and voice leg readiness...");
    if !profile.voice.vowifi_enabled && !profile.voice.carrier_fallback_enabled {
        return Err(live_stage_error("voice_no_leg_enabled"));
    }

    if !cached_live_ims_register_ready(line_id, profile).await {
        run_live_ims_register_until(line_id, profile, access_network).await?;
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
/// registration is current, and submits the call to the per-line operator
/// session. That session owns and reuses the registered SIP channel, including
/// its protected local port and Security-Verify agreement. Dialog/media
/// progress is reported asynchronously through the returned follow-up channel.
///
/// The RTP media loop itself is intentionally reserved: this function
/// establishes the signaling dialog and negotiates the codec, then hands the
/// media session to the (pluggable) audio backend via the reserved
/// [`voice::AudioSource`]/[`voice::AudioSink`] interfaces. Until a backend is
/// bound, media flows as silence.
/// Place a call using one line's network overrides.
pub async fn place_live_voice_call_for_line(
    line_id: &str,
    callee: &str,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveCallResult, LiveStageError> {
    if line_id.trim().is_empty() {
        return Err(live_stage_error("line_id_required"));
    }
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("voice_identity_unavailable"))?;
    // Resolve the carrier from THIS line's SIM, so a second line does not place
    // the call through the first line's operator profile.
    let identity = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("voice_identity_unavailable"))?;
    let profile_match = resolve_profile_for_line(
        line_id,
        identity.imsi.trim(),
        Some(identity.operator_id.trim()),
    )
    .ok_or_else(|| live_stage_error("voice_profile_unmatched"))?;
    let profile = profile_match.profile;

    if !profile.voice.vowifi_enabled {
        return Err(live_stage_error("voice_vowifi_leg_disabled"));
    }

    match tokio::time::timeout(
        LIVE_VOICE_INVITE_TOTAL_TIMEOUT,
        place_live_voice_call_for_profile(line_id, profile, callee, access_network),
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
    line_id: &str,
    profile: &'static CarrierProfile,
    callee: &str,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveCallResult, LiveStageError> {
    // Reject malformed destinations synchronously instead of reporting a
    // queued call which the operator task can only fail asynchronously.
    let _ = sip_phone_user(callee)?;
    if !cached_live_ims_register_ready(line_id, profile).await {
        info!(
            profile_id = profile.meta.profile_id,
            "VoWiFi voice call refreshing IMS registration before INVITE"
        );
        run_live_ims_register_until(line_id, profile, access_network).await?;
    }
    // REGISTER transfers ownership of the protected socket to this per-line
    // operator session. Opening a second socket on the same negotiated local
    // port fails (and using an ephemeral port would bypass the IPsec policy),
    // so every post-registration dialog must go through this link.
    let link = super::operator::operator_link_for_line(line_id);
    if !link.is_available() {
        return Err(live_stage_error("voice_operator_channel_unavailable"));
    }

    // The direct HTTP call has no Asterisk media endpoint. Give the operator
    // relay a loopback sink; its network-facing side still binds the tunnel's
    // assigned inner address. A future local audio backend can own this stable
    // endpoint without changing protected SIP signaling.
    let trunk_local_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let media_port = live_voice_media_port();
    let audio_endpoint = SocketAddr::new(trunk_local_ip, media_port);
    let audio = voice::build_mo_audio_offer(
        profile,
        &trunk_local_ip.to_string(),
        voice::SdpAddrType::Ip4,
        media_port,
    );
    let offered_codecs = audio
        .codecs
        .iter()
        .map(|codec| codec.codec)
        .collect::<Vec<_>>();
    let trace_id = format!("voice-mo-{}", hex_token(8));
    let call_id = format!("{}@simadmin", hex_token(16));
    let outcome = voice::MoCallSipOutcome {
        trace_id,
        call_id: call_id.clone(),
        sip_status: 0,
        invite_state: voice::SipInviteState::Queued,
        call_state: voice::CallState::Dialing,
        negotiated_codec: None,
        failure_cause: None,
    };
    let offer = MediaOffer {
        audio,
        audio_endpoint,
        video: None,
        dtmf: DtmfCapabilities {
            rtp_event: None,
            sip_info: true,
            preferred: DtmfSource::SipInfo,
        },
    };
    let mut events = link.subscribe_events();
    link.send_command(OperatorCommand::StartCall {
        call_id: call_id.clone(),
        caller: "simadmin".to_string(),
        callee: callee.to_string(),
        trunk_local_ip,
        offer,
    })
    .map_err(|_| live_stage_error("voice_operator_command_unavailable"))?;

    info!(
        profile_id = profile.meta.profile_id,
        call_id, media_port, "VoWiFi MO voice INVITE queued on registered operator channel"
    );

    let (tx, rx) = mpsc::unbounded_channel();
    let mut current = outcome.clone();
    let timeout_link = link.clone();
    let timeout_call_id = call_id.clone();
    tokio::spawn(async move {
        let setup_timeout = tokio::time::sleep(LIVE_VOICE_INVITE_TOTAL_TIMEOUT);
        tokio::pin!(setup_timeout);
        let mut awaiting_answer = true;
        loop {
            let event = tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = &mut setup_timeout, if awaiting_answer => {
                    let _ = timeout_link.send_command(OperatorCommand::CancelCall {
                        call_id: timeout_call_id.clone(),
                    });
                    let mut timed_out = current.clone();
                    timed_out.invite_state = voice::SipInviteState::Failed;
                    timed_out.call_state = voice::CallState::Failed;
                    timed_out.failure_cause = Some("voice_invite_response_timeout".to_string());
                    let _ = tx.send(LiveCallFollowupFrame { outcome: timed_out });
                    break;
                }
            };
            let Some((next, terminal)) =
                operator_event_call_outcome(&current, &offered_codecs, &event)
            else {
                continue;
            };
            awaiting_answer = next.call_state != voice::CallState::Active;
            current = next;
            let _ = tx.send(LiveCallFollowupFrame {
                outcome: current.clone(),
            });
            if terminal {
                break;
            }
        }
    });

    Ok(LiveCallResult {
        outcome,
        followup: rx,
    })
}

fn operator_event_call_outcome(
    seed: &voice::MoCallSipOutcome,
    offered_codecs: &[voice::AudioCodec],
    event: &OperatorEvent,
) -> Option<(voice::MoCallSipOutcome, bool)> {
    let event_call_id = match event {
        OperatorEvent::Started { .. } | OperatorEvent::Connected { .. } => return None,
        OperatorEvent::Provisional { call_id, .. }
        | OperatorEvent::Answered { call_id, .. }
        | OperatorEvent::Rejected { call_id, .. }
        | OperatorEvent::Unavailable { call_id }
        | OperatorEvent::Ended { call_id }
        | OperatorEvent::Cancelled { call_id } => call_id,
        OperatorEvent::Incoming { .. }
        | OperatorEvent::Renegotiate { .. }
        | OperatorEvent::Dtmf { .. }
        | OperatorEvent::TransferResponse { .. }
        | OperatorEvent::TransferNotify { .. } => return None,
    };
    if event_call_id != &seed.call_id {
        return None;
    }

    let mut outcome = seed.clone();
    let terminal = match event {
        OperatorEvent::Provisional { status, .. } => {
            outcome.sip_status = *status;
            outcome.invite_state = if *status == 183 {
                voice::SipInviteState::EarlyMedia
            } else {
                voice::SipInviteState::Ringing
            };
            outcome.call_state = voice::CallState::Ringing;
            false
        }
        OperatorEvent::Answered { body, .. } => {
            outcome.sip_status = 200;
            outcome.invite_state = voice::SipInviteState::Confirmed;
            outcome.call_state = voice::CallState::Active;
            outcome.negotiated_codec = voice::parse_audio_sdp(body).ok().and_then(|answer| {
                answer
                    .codecs
                    .iter()
                    .find(|remote| offered_codecs.contains(&remote.codec))
                    .map(|remote| remote.codec)
            });
            false
        }
        OperatorEvent::Rejected {
            status, diagnostic, ..
        } => {
            outcome.sip_status = *status;
            outcome.invite_state = voice::SipInviteState::Failed;
            outcome.call_state = voice::CallState::Failed;
            outcome.failure_cause = Some(diagnostic.code.to_string());
            true
        }
        OperatorEvent::Unavailable { .. } => {
            outcome.invite_state = voice::SipInviteState::Failed;
            outcome.call_state = voice::CallState::Failed;
            outcome.failure_cause = Some("vowifi_operator_unavailable".to_string());
            true
        }
        OperatorEvent::Ended { .. } => {
            outcome.invite_state = voice::SipInviteState::Terminated;
            outcome.call_state = voice::CallState::Ended;
            true
        }
        OperatorEvent::Cancelled { .. } => {
            outcome.invite_state = voice::SipInviteState::Failed;
            outcome.call_state = voice::CallState::Failed;
            outcome.failure_cause = Some("cancelled".to_string());
            true
        }
        OperatorEvent::Started { .. }
        | OperatorEvent::Connected { .. }
        | OperatorEvent::Incoming { .. }
        | OperatorEvent::Renegotiate { .. }
        | OperatorEvent::Dtmf { .. }
        | OperatorEvent::TransferResponse { .. }
        | OperatorEvent::TransferNotify { .. } => return None,
    };
    Some((outcome, terminal))
}

/// The local RTP media port used for the SDP offer. A fixed port in the dynamic
/// range keeps the reserved media path deterministic; the real backend may
/// override this when it binds the socket.
fn live_voice_media_port() -> u16 {
    40000
}

fn ims_register_ready_cache() -> &'static Mutex<HashMap<String, LiveImsRegisterReady>> {
    LIVE_IMS_REGISTER_READY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_security_verify_cache() -> &'static Mutex<HashMap<String, LiveImsSecurityVerify>> {
    LIVE_IMS_SECURITY_VERIFY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_channel_cache() -> &'static Mutex<HashMap<String, LiveImsChannel>> {
    LIVE_IMS_CHANNEL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn xcap_binding_cache() -> &'static Mutex<HashMap<String, LiveXcapBinding>> {
    LIVE_XCAP_BINDING.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn live_xcap_access_for_line(line_id: &str) -> Option<XcapAccessContext> {
    let binding = xcap_binding_cache().lock().await.get(line_id).cloned()?;
    Some(XcapAccessContext {
        access: ImsRegistrationAccess::Vowifi,
        profile: binding.profile,
        local_address: binding.local_address,
        digest: Arc::new(VowifiXcapDigestProvider {
            line_id: line_id.to_string(),
            username: binding.username,
        }),
    })
}

fn ims_register_variant_cache() -> &'static Mutex<HashMap<String, LiveImsRegisterSuccessVariant>> {
    LIVE_IMS_REGISTER_SUCCESS_VARIANT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ims_refresh_failure_cache() -> &'static Mutex<HashMap<String, LiveImsRefreshFailure>> {
    LIVE_IMS_REFRESH_FAILURE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record one completed refresh cycle. The caller must invoke this once after
/// its configured per-cycle retries are exhausted; counting individual socket
/// or header attempts would make the three-cycle policy equivalent to the old
/// three-identical-attempt policy.
pub(crate) async fn record_live_ims_refresh_failure(
    line_id: &str,
    reason: &str,
) -> LiveImsRefreshFailureDecision {
    let mut failures = ims_refresh_failure_cache().lock().await;
    let state = failures
        .entry(line_id.to_string())
        .or_insert_with(|| LiveImsRefreshFailure {
            consecutive_failures: 0,
            last_failure_reason: String::new(),
            rebuild_pending: false,
        });
    state.consecutive_failures = state
        .consecutive_failures
        .saturating_add(1)
        .min(LIVE_IMS_REFRESH_REBUILD_FAILURES);
    state.last_failure_reason = reason.trim().to_string();
    if state.rebuild_pending {
        return LiveImsRefreshFailureDecision::RebuildPending;
    }
    if state.consecutive_failures >= LIVE_IMS_REFRESH_REBUILD_FAILURES {
        LiveImsRefreshFailureDecision::RebuildAccess
    } else {
        LiveImsRefreshFailureDecision::Retry
    }
}

/// Mark a threshold breach as deferred because an active/held call still owns
/// the current operator session. The state remains line-scoped until the call
/// monitor observes that the line has no protected call left.
pub(crate) async fn mark_live_ims_refresh_rebuild_pending(line_id: &str) {
    let mut failures = ims_refresh_failure_cache().lock().await;
    let state = failures
        .entry(line_id.to_string())
        .or_insert_with(|| LiveImsRefreshFailure {
            consecutive_failures: LIVE_IMS_REFRESH_REBUILD_FAILURES,
            last_failure_reason: "vowifi_refresh_rebuild_pending".to_string(),
            rebuild_pending: false,
        });
    state.rebuild_pending = true;
}

pub(crate) async fn live_ims_refresh_failure_count_for_line(line_id: &str) -> u8 {
    ims_refresh_failure_cache()
        .lock()
        .await
        .get(line_id)
        .map(|state| state.consecutive_failures)
        .unwrap_or(0)
}

pub(crate) async fn live_ims_refresh_rebuild_pending_for_line(line_id: &str) -> bool {
    ims_refresh_failure_cache()
        .lock()
        .await
        .get(line_id)
        .is_some_and(|state| state.rebuild_pending)
}

pub(crate) async fn clear_live_ims_refresh_failure_for_line(line_id: &str) {
    ims_refresh_failure_cache().lock().await.remove(line_id);
}

async fn record_live_ims_register_ready(
    line_id: &str,
    profile: &'static CarrierProfile,
    sms_capability_advertised: bool,
    registration: RegisteredImsContext,
) {
    let ttl = registration.lease.refresh_after;
    let expires_seconds = registration.lease.expires_seconds;
    ims_register_ready_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsRegisterReady {
            profile_id: profile.meta.profile_id,
            expires_at: Instant::now() + ttl,
            registration,
            sms_capability_advertised,
            receiver_transport: profile.sms.receiver_transport,
        },
    );
    // Any successful REGISTER, including one initiated by SMS/voice after a
    // stale lease, proves that this line recovered. Do not let an old refresh
    // failure count force a later access rebuild.
    clear_live_ims_refresh_failure_for_line(line_id).await;
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

/// The REGISTER cache expires at the lease's refresh deadline (11/12 of the
/// network lifetime), before the installed operator channel reaches its hard
/// expiry. The restore scheduler uses this signal to replace the registration
/// without creating an avoidable signaling gap every lease interval.
pub(crate) async fn live_ims_registration_refresh_due_for_line(line_id: &str) -> bool {
    ims_register_ready_cache()
        .lock()
        .await
        .get(line_id)
        .is_none_or(|ready| ready.expires_at <= Instant::now())
}

async fn cached_live_ims_registration(
    line_id: &str,
    profile: &'static CarrierProfile,
) -> Option<RegisteredImsContext> {
    ims_register_ready_cache()
        .lock()
        .await
        .get(line_id)
        .filter(|ready| ready.profile_id == profile.meta.profile_id)
        .filter(|ready| ready.expires_at > Instant::now())
        .map(|ready| ready.registration.clone())
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
    registration: &RegisteredImsContext,
) {
    let Some(value) = security_verify.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    ims_security_verify_cache().lock().await.insert(
        line_id.to_string(),
        LiveImsSecurityVerify {
            profile_id: profile.meta.profile_id,
            expires_at: Instant::now() + registration.lease.refresh_after,
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
async fn record_live_ims_channel(
    line_id: &str,
    profile: &'static CarrierProfile,
    identity: crate::connectivity::core::context::ImsIdentity,
    channel: SipChannel,
    security_verify: Option<String>,
    registration: RegisteredImsContext,
    register_context: LiveRegisterRequestContext,
    register_variant: LiveRegisterHeaderVariant,
    next_register_cseq: u32,
) {
    let route = channel.route();
    let expires_at = Instant::now() + registration.lease.expires_after;
    let media_route_installer: Option<Arc<dyn super::operator::MediaRouteInstaller>> =
        cached_tun_gateway(line_id, profile)
            .await
            .ok()
            .map(|gateway| gateway as Arc<dyn super::operator::MediaRouteInstaller>);
    let media_operator_creator: Option<Arc<dyn OperatorSocketCreator>> =
        ue_socket_context_for_line(line_id).map(|context| {
            Arc::new(super::operator::UeWorkerOperatorSocketCreator::new(
                context.worker,
            )) as Arc<dyn OperatorSocketCreator>
        });
    xcap_binding_cache().lock().await.insert(
        line_id.to_string(),
        LiveXcapBinding {
            profile,
            local_address: route.local_addr.ip(),
            username: identity.private_user.clone(),
        },
    );
    super::operator::install_registered_channel(
        super::operator::RegisteredVoiceContext {
            line_id: line_id.to_string(),
            profile_id: profile.meta.profile_id,
            identity,
            route,
            registration,
            security_verify: security_verify.clone(),
            pani: build_p_access_network_info(profile),
            user_agent: build_live_user_agent(profile, LiveUserAgentFormat::ProfileDefault),
            expires_at,
            tcp_keepalive_interval: (profile.ims.tcp_keepalive_seconds != 0)
                .then(|| Duration::from_secs(u64::from(profile.ims.tcp_keepalive_seconds))),
            options_ping_interval: (profile.ims.options_ping_interval_seconds != 0)
                .then(|| Duration::from_secs(u64::from(profile.ims.options_ping_interval_seconds))),
            unregister: Some(Arc::new(VowifiUnregisterFactory {
                line_id: line_id.to_string(),
                profile,
                context: register_context,
                variant: register_variant,
                next_cseq: next_register_cseq,
                security_verify: security_verify.clone(),
            })),
            media_route_installer,
            media_interface: Some(tun_name_for_line(&live_runtime_config().tun_name, line_id)),
            media_operator_creator,
        },
        channel,
    )
    .await;
}

async fn clear_live_ims_channel(line_id: &str, profile: &'static CarrierProfile) {
    super::operator::abort_profile(line_id, profile.meta.profile_id).await;
    let mut cache = ims_channel_cache().lock().await;
    if cache
        .get(line_id)
        .is_some_and(|channel| channel.profile_id == profile.meta.profile_id)
    {
        cache.remove(line_id);
    }
    let mut xcap = xcap_binding_cache().lock().await;
    if xcap
        .get(line_id)
        .is_some_and(|binding| binding.profile.meta.profile_id == profile.meta.profile_id)
    {
        xcap.remove(line_id);
    }
}

/// Tear down one line's live runtime, leaving every other line untouched.
pub async fn clear_live_runtime_for_line(line_id: &str) {
    let unregister = super::operator::disconnect_line(line_id).await;
    info!(result = ?unregister, "VoWiFi explicit IMS unregister finished");
    if let Some(channel) = ims_channel_cache().lock().await.remove(line_id) {
        channel.channel.abort();
    }
    ims_register_ready_cache().lock().await.remove(line_id);
    ims_security_verify_cache().lock().await.remove(line_id);
    ims_register_variant_cache().lock().await.remove(line_id);
    clear_live_ims_refresh_failure_for_line(line_id).await;
    xcap_binding_cache().lock().await.remove(line_id);
    if let Some(gateway) = tun_gateway_cache().lock().await.remove(line_id) {
        gateway.shutdown();
    }
    forget_live_network_overrides(line_id);
}

async fn clear_live_ims_session(line_id: &str, profile: &'static CarrierProfile) {
    clear_live_ims_channel(line_id, profile).await;

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

/// Send an IMS SMS using one line's network overrides.
pub async fn send_live_sms_over_ims_for_line(
    line_id: &str,
    recipient: &str,
    text: &str,
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveSmsSendResult, LiveStageError> {
    if line_id.trim().is_empty() {
        return Err(live_stage_error("line_id_required"));
    }
    let conn = zbus::Connection::system()
        .await
        .map_err(|_| live_stage_error("sms_identity_unavailable"))?;
    // Resolve the carrier from THIS line's SIM, so a second line does not send
    // through the first line's operator profile.
    let identity = line_sim_identity(line_id, &conn)
        .await
        .ok_or_else(|| live_stage_error("sms_identity_unavailable"))?;
    let profile_match = resolve_profile_for_line(
        line_id,
        identity.imsi.trim(),
        Some(identity.operator_id.trim()),
    )
    .ok_or_else(|| live_stage_error("sms_profile_unmatched"))?;
    let profile = profile_match.profile;

    match tokio::time::timeout(
        LIVE_SMS_SEND_TOTAL_TIMEOUT,
        send_live_sms_over_ims_for_profile(
            &conn,
            line_id,
            profile,
            recipient,
            text,
            access_network,
        ),
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<LiveSmsSendResult, LiveStageError> {
    if !cached_live_ims_register_ready(line_id, profile).await {
        info!(
            profile_id = profile.meta.profile_id,
            "VoWiFi SMS send refreshing IMS registration before MESSAGE"
        );
        run_live_ims_register_until(line_id, profile, access_network).await?;
    }
    let gateway = cached_tun_gateway(line_id, profile).await?;
    let route = gateway
        .ims_client_tcp_route()
        .map_err(|error| live_stage_error(error.reason()))?;
    if route.profile_id != profile.meta.profile_id {
        return Err(live_stage_error("sms_ims_policy_profile_mismatch"));
    }

    let sim_info = line_sim_info(line_id, conn)
        .await
        .ok_or_else(|| live_stage_error("sms_smsc_unavailable"))?;
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
    let variants = live_sms_request_uri_variants(line_id, profile, recipient, service_center)?;

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
            run_live_ims_register_until(line_id, profile, access_network).await?;
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<String, LiveStageError> {
    let mut last_error = None;
    for pcscf_addr in register_pcscf_candidates(gateway) {
        match run_register_exchange_with_pcscf(
            line_id,
            profile,
            gateway,
            pcscf_addr,
            access_network,
        )
        .await
        {
            Ok(response) => return Ok(response),
            Err(error) => {
                warn!(
                    reason = error.reason.as_str(),
                    "IMS REGISTER candidate failed"
                );
                // A terminal rejection is the same answer on every P-CSCF;
                // stop instead of cycling the remaining candidates.
                if live_register_error_is_terminal(&error) {
                    return Err(error);
                }
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<String, LiveStageError> {
    let mut last_error = None;
    let mut attempt_count = 0usize;
    let variants = live_register_header_variants_for_attempt(line_id, profile).await;
    for base_variant in variants {
        let mut variant = base_variant;
        loop {
            if attempt_count >= LIVE_IMS_REGISTER_MAX_VARIANT_ATTEMPTS {
                warn!(
                    attempts = attempt_count,
                    "IMS REGISTER variant budget exhausted"
                );
                return Err(last_error
                    .unwrap_or_else(|| live_stage_error("ims_register_variant_budget_exhausted")));
            }
            attempt_count += 1;
            match run_register_exchange_with_pcscf_variant(
                line_id,
                profile,
                gateway,
                pcscf_addr,
                variant,
                access_network,
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
                        register_attempt = attempt_count,
                        reason = err.reason.as_str(),
                        "IMS REGISTER header variant failed"
                    );
                    // A terminal rejection cannot be cleared by another shape or
                    // P-CSCF; stop the whole ladder instead of exhausting it.
                    if live_register_error_is_terminal(&err) {
                        return Err(err);
                    }
                    // Response-driven variants are cumulative. In particular,
                    // once 421/494 has required sec-agree, a following 400 is
                    // retried with the same declaration plus an empty AKA
                    // Authorization instead of dropping back to a partial shape.
                    if let Some(upgraded) =
                        next_dynamic_live_register_variant(profile, variant, &err)
                    {
                        info!(
                            previous_register_variant = variant.label,
                            register_variant = upgraded.label,
                            register_attempt = attempt_count + 1,
                            sip_status = live_register_error_status(&err),
                            "Retrying IMS REGISTER with cumulative response-driven variant"
                        );
                        last_error = Some(err);
                        variant = upgraded;
                        continue;
                    }
                    last_error = Some(err);
                    break;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_register_variant_missing")))
}

/// Advance one response-driven REGISTER shape while preserving all choices the
/// registrar has already made. The ordering mirrors the field-verified VoLTE
/// path: declare sec-agree first, add empty AKA after a 400, and only then probe
/// bounded Security-Client formatting variants.
fn next_dynamic_live_register_variant(
    profile: &CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    error: &LiveStageError,
) -> Option<LiveRegisterHeaderVariant> {
    sec_agree_retry_variant(profile, variant, error)
        .or_else(|| sec_agree_empty_aka_retry_variant(profile, variant, error))
        .or_else(|| sec_agree_compact_security_retry_variant(profile, variant, error))
        .or_else(|| sec_agree_minimal_security_retry_variant(profile, variant, error))
}

/// Upgrade a variant that was refused with `421 Extension Required: sec-agree`
/// or `494 Security Agreement Required`.
///
/// An `auto` profile starts challenge-first. If the registrar explicitly
/// demands RFC 3329, retry the same shape with Security-Client and both
/// Require/Proxy-Require. An explicit database/catalog `disabled` is final and
/// can never be upgraded by the candidate ladder.
fn sec_agree_retry_variant(
    profile: &CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    error: &LiveStageError,
) -> Option<LiveRegisterHeaderVariant> {
    if profile.ims.register.sec_agree_mode == "disabled"
        || error.register_auth_rounds != 0
        || !error.server_required_sec_agree
        || variant.force_sec_agree_headers
    {
        return None;
    }
    Some(LiveRegisterHeaderVariant {
        label: "catalog_v7_sec_agree_required",
        force_sec_agree_headers: true,
        server_required_sec_agree: true,
        suppress_sec_agree_headers: false,
        include_security_client: true,
        ..variant
    })
}

/// The measured Maxis VoWiFi sequence is 421 -> fully declared sec-agree ->
/// 400. At that point the request shape is accepted far enough that the core is
/// waiting for the AKA identity hint. Add URI-first empty AKA without dropping
/// Security-Client, Require, Proxy-Require, routing or access headers.
fn sec_agree_empty_aka_retry_variant(
    profile: &CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    error: &LiveStageError,
) -> Option<LiveRegisterHeaderVariant> {
    (profile.ims.register.sec_agree_mode != "disabled"
        && variant.server_required_sec_agree
        && variant.force_sec_agree_headers
        && variant.include_security_client
        && variant.initial_authorization == LiveInitialAuthorizationFormat::None
        && error.register_auth_rounds == 0
        && live_register_error_status(error) == Some(400))
    .then_some(LiveRegisterHeaderVariant {
        label: "catalog_v7_sec_agree_required_aka_empty_uri_first",
        initial_authorization: LiveInitialAuthorizationFormat::AkaEmptyUriFirst,
        ..variant
    })
}

/// If the cumulative empty-AKA request is still rejected as malformed, retain
/// Authorization and sec-agree while probing the compact full mechanism syntax.
fn sec_agree_compact_security_retry_variant(
    profile: &CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    error: &LiveStageError,
) -> Option<LiveRegisterHeaderVariant> {
    (profile.ims.register.sec_agree_mode != "disabled"
        && variant.server_required_sec_agree
        && variant.force_sec_agree_headers
        && variant.include_security_client
        && variant.initial_authorization != LiveInitialAuthorizationFormat::None
        && variant.security_client_format == LiveSecurityClientFormat::FullSpaced
        && error.register_auth_rounds == 0
        && live_register_error_status(error) == Some(400))
    .then_some(LiveRegisterHeaderVariant {
        label: "catalog_v7_sec_agree_required_aka_compact_security",
        security_client_format: LiveSecurityClientFormat::FullCompact,
        ..variant
    })
}

/// Final response-driven format probe. There is deliberately no transition out
/// of MinimalSpaced, so repeated 400 responses cannot cycle back to an earlier
/// shape; the outer global budget is an additional safety valve.
fn sec_agree_minimal_security_retry_variant(
    profile: &CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    error: &LiveStageError,
) -> Option<LiveRegisterHeaderVariant> {
    (profile.ims.register.sec_agree_mode != "disabled"
        && variant.server_required_sec_agree
        && variant.force_sec_agree_headers
        && variant.include_security_client
        && variant.initial_authorization != LiveInitialAuthorizationFormat::None
        && variant.security_client_format == LiveSecurityClientFormat::FullCompact
        && error.register_auth_rounds == 0
        && live_register_error_status(error) == Some(400))
    .then_some(LiveRegisterHeaderVariant {
        label: "catalog_v7_sec_agree_required_aka_minimal_security",
        security_client_format: LiveSecurityClientFormat::MinimalSpaced,
        ..variant
    })
}

fn live_register_header_variants(
    profile: &'static CarrierProfile,
) -> Vec<LiveRegisterHeaderVariant> {
    #[cfg(test)]
    match profile.ims.register.live_header_variant_set {
        "ee_ims_features" => return GB_EE_REGISTER_HEADER_VARIANTS.to_vec(),
        "standard_ims_features" => return LIVE_REGISTER_HEADER_VARIANTS.to_vec(),
        _ => {}
    }

    let register = profile.ims.register;
    let initial_authorization = match register.initial_authorization {
        "aka_empty" | "digest_empty" | "implementation_variant" => {
            LiveInitialAuthorizationFormat::AkaEmpty
        }
        _ => LiveInitialAuthorizationFormat::None,
    };
    // `auto` is challenge driven: do not emit Security-Client until a 421/494
    // or a concrete Security-Server offer proves the network expects it.
    let include_security_client =
        register.sec_agree_mode == "required" && !register.security_client_mechanisms.is_empty();
    let contact_features = if register.include_mmtel_features {
        LiveContactFeatureSet::MmtelSmsSipInstance
    } else {
        LiveContactFeatureSet::SmsOnly
    };
    let pani = if register.include_pani_initial || register.include_pani_authenticated {
        LivePaniFormat::ProfileDefault
    } else {
        LivePaniFormat::Omit
    };
    let compact_register = std::env::var("SIMADMIN_COMPACT_REGISTER").is_ok();
    let header_profile = LiveRegisterHeaderProfile {
        contact_features,
        include_accept_contact: false,
        include_p_preferred_identity: register.include_p_preferred_identity,
        visited_network: if register.include_visited_network {
            LiveVisitedNetworkFormat::QuotedHome
        } else {
            LiveVisitedNetworkFormat::Omit
        },
        pani,
        include_cellular_network_info: register.enable_cellular_network_info && !compact_register,
        user_agent: LiveUserAgentFormat::ProfileDefault,
        compact_register,
    };
    let exact = LiveRegisterHeaderVariant {
        label: register.live_header_variant_set,
        force_sec_agree_headers: register.sec_agree_mode == "required",
        server_required_sec_agree: false,
        suppress_sec_agree_headers: false,
        include_route_header: register.include_route_header,
        include_security_client,
        initial_authorization,
        security_client_format: LiveSecurityClientFormat::FullSpaced,
        request_uri: match register.request_uri_policy {
            "home_domain" => LiveRegisterRequestUri::HomeDomain,
            "pcscf" => LiveRegisterRequestUri::PcscfSocket,
            _ => LiveRegisterRequestUri::HomeRegistrar,
        },
        identity_format: LiveRegisterIdentityFormat::ImsiHomeDomain,
        header_profile,
    };

    // The exact database/catalog policy is always attempted first. Optional
    // candidates are bounded and never reinterpret an explicit false as a
    // missing field.
    let mut variants = vec![exact];
    if register.sec_agree_mode == "required" {
        variants.push(LiveRegisterHeaderVariant {
            label: "catalog_v7_challenge_first",
            force_sec_agree_headers: false,
            server_required_sec_agree: false,
            suppress_sec_agree_headers: true,
            include_route_header: register.include_route_header,
            include_security_client: false,
            initial_authorization,
            security_client_format: LiveSecurityClientFormat::FullSpaced,
            request_uri: exact.request_uri,
            identity_format: exact.identity_format,
            header_profile,
        });
    }

    let allow_ipcc_fallback = matches!(
        register.live_header_variant_set,
        "iphone_ipcc_fallback" | "catalog_v7_ipcc_access_fallback"
    );
    if allow_ipcc_fallback {
        variants.push(LiveRegisterHeaderVariant {
            label: "catalog_v7_ipcc_access_baseline",
            force_sec_agree_headers: exact.force_sec_agree_headers,
            server_required_sec_agree: false,
            suppress_sec_agree_headers: false,
            include_route_header: register.include_route_header,
            include_security_client,
            initial_authorization,
            security_client_format: LiveSecurityClientFormat::FullSpaced,
            request_uri: exact.request_uri,
            identity_format: exact.identity_format,
            header_profile: LiveRegisterHeaderProfile {
                contact_features: LiveContactFeatureSet::SmsOnly,
                include_accept_contact: false,
                include_p_preferred_identity: register.include_p_preferred_identity,
                visited_network: if register.include_visited_network {
                    LiveVisitedNetworkFormat::QuotedHome
                } else {
                    LiveVisitedNetworkFormat::Omit
                },
                pani,
                include_cellular_network_info: register.enable_cellular_network_info
                    && !compact_register,
                user_agent: LiveUserAgentFormat::ProfileDefault,
                compact_register,
            },
        });
    }
    variants
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
            && cached.profile_address == profile as *const CarrierProfile as usize
            && cached.captured_at.elapsed() <= LIVE_IMS_REGISTER_MAX_TTL
    }) else {
        return variants;
    };

    let Some(exact) = variants.first().copied() else {
        return variants;
    };
    let success = cached.variant;
    if success.label == exact.label {
        return variants;
    }
    let mut ordered = Vec::with_capacity(variants.len() + 1);
    ordered.push(exact);
    ordered.push(success);
    ordered.extend(
        variants
            .iter()
            .copied()
            .filter(|variant| variant.label != exact.label && variant.label != success.label),
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
            profile_address: profile as *const CarrierProfile as usize,
            variant,
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
    access_network: &ImsAccessNetworkRuntime,
) -> Result<String, LiveStageError> {
    let target = SocketAddr::new(pcscf_addr, profile.ims.local_port);
    let transport = ims_transport(profile);
    let ue_socket = ue_socket_context_for_line(line_id);
    let socket = connect_sip_socket(
        gateway.inner_addr(),
        target,
        profile.ims.local_port,
        transport,
        Some(gateway.tun_name()),
        ue_socket.as_ref(),
    )
    .await?;
    let local_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(_) => {
            socket.abort();
            return Err(live_stage_error("ims_sip_local_addr_unavailable"));
        }
    };
    let route = crate::connectivity::core::context::ImsRoute {
        local_addr,
        pcscf_addr: target,
        transport,
    };
    let mut channel = SipChannel::new(socket, Vec::new(), route, None);

    match run_register_exchange_on_connected_stream(
        line_id,
        profile,
        &mut channel,
        gateway,
        local_addr,
        pcscf_addr,
        variant,
        access_network,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            channel.abort();
            Err(err)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_register_exchange_on_connected_stream(
    line_id: &str,
    profile: &'static CarrierProfile,
    channel: &mut SipChannel,
    gateway: &TunGatewayRuntime,
    local_addr: SocketAddr,
    pcscf_addr: IpAddr,
    variant: LiveRegisterHeaderVariant,
    access_network_runtime: &ImsAccessNetworkRuntime,
) -> Result<String, LiveStageError> {
    let identity = live_ims_register_identity(line_id, profile, variant.identity_format).await?;
    let identity_shape = identity.shape;
    let access_network = (profile.ims.register.enable_cellular_network_info
        && variant.header_profile.include_cellular_network_info)
        .then(|| access_network_runtime.context(profile.ims.register.access_network_info))
        .flatten();
    if profile.ims.register.enable_cellular_network_info
        && variant.header_profile.include_cellular_network_info
        && profile.ims.register.cni_identity_policy == AccessIdentityPolicy::RequiredDynamic
        && access_network.is_none()
    {
        return Err(live_stage_error("ims_cni_required_dynamic_unavailable"));
    }
    if profile.ims.register.pani_identity_policy == AccessIdentityPolicy::RequiredDynamic
        && !matches!(variant.header_profile.pani, LivePaniFormat::Omit)
        && (profile.ims.register.include_pani_initial
            || profile.ims.register.include_pani_authenticated)
    {
        // The current VoWiFi adapter has a dynamic cellular snapshot for CNI,
        // but no trustworthy Wi-Fi BSSID/node-id provider for PANI.
        return Err(live_stage_error("ims_pani_required_dynamic_unavailable"));
    }
    let mut context = LiveRegisterRequestContext::new_for_line(
        line_id, profile, identity, local_addr, pcscf_addr,
    )?;
    context.access_network = access_network;
    let request = context.build_initial_request(profile, variant);
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
        cellular_network_info_present = request.contains("Cellular-Network-Info:"),
        user_agent_format = variant.header_profile.user_agent.label(),
        request_uri = variant.request_uri.label(),
        identity_format = variant.identity_format.label(),
        sec_agree_headers_present = sec_agree_headers_required(
            profile,
            variant.force_sec_agree_headers,
            variant.suppress_sec_agree_headers,
        ),
        contact_feature_count = context.contact_feature_count(profile, variant.header_profile),
        sms_over_ip_advertised = request.to_ascii_lowercase().contains("+g.3gpp.smsip"),
        local_port = local_addr.port(),
        expected_header_port = profile.ims.local_port,
        sip_instance_present = matches!(
            variant.header_profile.contact_features,
            LiveContactFeatureSet::MmtelSmsSipInstance
        ),
        security_client_present = variant.include_security_client,
        "IMS REGISTER request metadata prepared"
    );
    let mut authenticator = VowifiRegisterAuthenticator {
        line_id,
        profile,
        gateway,
        context: &mut context,
        variant,
        last_security_server: None,
        resync_sent: false,
        last_error: None,
    };
    let registration =
        match run_register_observed(channel, request.as_bytes(), &mut authenticator).await {
            Ok(registration) => registration,
            Err(failure) => {
                return Err(authenticator
                    .last_error
                    .take()
                    .unwrap_or_else(|| map_shared_register_failure(&failure)))
            }
        };
    let response = String::from_utf8(registration.response)
        .map_err(|_| live_stage_error("ims_register_response_not_utf8"))?;
    let summary = ims::parse_sip_response(&response, &live_ims_target(line_id, profile).realm)
        .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
    let artifacts = RegisterArtifacts::parse(response.as_bytes());
    info!(
        status_code = summary.status_code,
        reason = summary.reason.as_str(),
        auth_rounds = registration.auth_rounds,
        expires_seconds = artifacts.expires_seconds,
        service_route_present = artifacts.service_route.is_some(),
        service_route_count = artifacts.service_route_count,
        associated_uri_count = artifacts.associated_uris.len(),
        contact_binding_count = artifacts.contact_binding_count,
        contact_expiry_ambiguous = artifacts.contact_expiry_ambiguous,
        wildcard_contact_present = artifacts.wildcard_contact_present,
        security_server_offers = summary.security_server_offers.len(),
        warning_present = summary.warning_present,
        unsupported = ?summary.unsupported,
        require = ?summary.require,
        proxy_require = ?summary.proxy_require,
        "IMS REGISTER final response metadata received from shared engine"
    );
    Ok(response)
}

struct VowifiRegisterAuthenticator<'a> {
    line_id: &'a str,
    profile: &'static CarrierProfile,
    gateway: &'a TunGatewayRuntime,
    context: &'a mut LiveRegisterRequestContext,
    variant: LiveRegisterHeaderVariant,
    last_security_server: Option<(Vec<String>, Vec<LiveSecurityServerOffer>)>,
    resync_sent: bool,
    last_error: Option<LiveStageError>,
}

impl RegisterAuthenticator<SipChannel> for VowifiRegisterAuthenticator<'_> {
    async fn exchange_authenticated(
        &mut self,
        challenge_response: &[u8],
        cseq: u32,
        channel: &mut SipChannel,
    ) -> Result<Option<Vec<u8>>, ImsError> {
        let exchange = async {
            let response = std::str::from_utf8(challenge_response)
                .map_err(|_| live_stage_error("ims_register_response_not_utf8"))?;
            let mut challenge = parse_live_digest_challenge(response, &self.context.target.realm)?;
            reject_plain_digest_when_disabled(self.profile, &challenge)?;
            if challenge.security_server_offers.is_empty() {
                if let Some((values, offers)) = self.last_security_server.as_ref() {
                    challenge.security_server_values = values.clone();
                    challenge.security_server_offers = offers.clone();
                }
            } else {
                self.last_security_server = Some((
                    challenge.security_server_values.clone(),
                    challenge.security_server_offers.clone(),
                ));
            }
            info!(
                header_kind = challenge.header_kind,
                algorithm = challenge.algorithm.as_str(),
                qop_present = challenge.qop.is_some(),
                opaque_present = challenge.opaque.is_some(),
                security_server_offer_count = challenge.security_server_offers.len(),
                cseq,
                "IMS REGISTER digest challenge delegated by shared engine"
            );
            run_authenticated_register_exchange(
                self.line_id,
                self.profile,
                channel,
                self.gateway,
                self.context,
                &challenge,
                self.variant,
                cseq,
                &mut self.resync_sent,
            )
            .await
        }
        .await;

        match exchange {
            Ok(response) => Ok(Some(response.into_bytes())),
            Err(error) => {
                let registration_loss = classify_vowifi_register_error(error.reason.as_str());
                self.last_error = Some(error.with_registration_loss(registration_loss));
                Err(ImsError::new(
                    "vowifi_register_authenticated_exchange_failed",
                ))
            }
        }
    }

    async fn authenticated_request(
        &mut self,
        _challenge_response: &[u8],
        _cseq: u32,
    ) -> Result<Vec<u8>, ImsError> {
        Err(ImsError::new(
            "vowifi_register_default_exchange_unreachable",
        ))
    }

    async fn rebuild_register_with_min_expires(
        &mut self,
        _challenge_response: &[u8],
        cseq: u32,
        min_expires: u32,
        authenticated: bool,
    ) -> Result<Vec<u8>, ImsError> {
        if authenticated {
            // Authenticated 423 retries are handled inside the adapter-owned
            // exchange so the ESP-protected channel is reused; rebuilding over
            // the plain channel here would drop the security association.
            return Err(ImsError::new(
                "ims_register_authenticated_min_expires_unsupported",
            ));
        }
        // Re-send the same initial shape (including its empty-AKA
        // Authorization, if any) with a lease that satisfies Min-Expires.
        let initial_authorization = self
            .context
            .build_initial_authorization_header(self.profile, self.variant);
        let request = self.context.build_register_request_with_expires(
            self.profile,
            self.variant,
            cseq,
            initial_authorization.as_deref(),
            None,
            min_expires,
        );
        Ok(request.into_bytes())
    }
}

fn map_shared_register_failure(failure: &RegisterFailure) -> LiveStageError {
    let reason = match failure.error.code() {
        "ims_register_initial_send_failed" | "ims_register_authenticated_send_failed" => {
            "ims_register_write_failed"
        }
        "ims_register_initial_receive_failed" | "ims_register_authenticated_receive_failed" => {
            "ims_register_read_failed"
        }
        "ims_register_authenticated_unexpected_status" => "ims_register_unexpected_status",
        "ims_register_auth_rejected" => "ims_register_auth_rejected",
        "ims_register_initial_unexpected_status" => "ims_register_initial_unexpected_status",
        // 423 is negotiated inside the shared engine; these escape only after
        // the bounded retry loop, so treat them as the unexpected-status
        // family rather than inventing a new terminal reason.
        "ims_register_initial_min_expires_invalid"
        | "ims_register_initial_min_expires_exhausted"
        | "ims_register_initial_min_expires_unsupported" => {
            "ims_register_initial_unexpected_status"
        }
        "ims_register_authenticated_min_expires_invalid"
        | "ims_register_authenticated_min_expires_exhausted"
        | "ims_register_authenticated_min_expires_unsupported" => "ims_register_unexpected_status",
        "sip_status_line_missing" | "sip_status_code_invalid" => {
            "ims_register_response_parse_failed"
        }
        other => other,
    };
    let mut error = live_registration_error(
        reason,
        RegistrationLossReason::from_register_failure(failure),
    );
    error.server_required_sec_agree = register_failure_demands_sec_agree(failure);
    error.register_auth_rounds = failure.auth_rounds;
    // Preserve the final SIP status in the reason so the candidate loops can
    // classify terminal rejections without touching the secret-bearing
    // response buffer.
    if let Some(status) = failure
        .response
        .as_deref()
        .and_then(|response| sip_frame::parse_status(response).ok())
    {
        error.reason = format!("{}:sip_status={status}", error.reason);
    }
    error
}

/// Recover the final SIP status embedded by `map_shared_register_failure`.
fn live_register_error_status(error: &LiveStageError) -> Option<u16> {
    error
        .reason
        .rsplit_once(":sip_status=")
        .and_then(|(_, suffix)| suffix.parse::<u16>().ok())
}

/// True when this REGISTER failure is final for the SIM/line: the core
/// rejected the identity, refused the request on policy, or sent a redirect
/// this project does not follow. No other header shape or P-CSCF can change
/// the answer, so abort the candidate ladder instead of burning attempts.
fn live_register_error_is_terminal(error: &LiveStageError) -> bool {
    if error.reason.starts_with("ims_register_auth_rejected") {
        // The bounded challenge rounds were exhausted: credentials were
        // rejected, not shaped wrong.
        return true;
    }
    live_register_error_status(error).is_some_and(status_is_terminal_register_failure)
}

/// True when the core rejected the unauthenticated REGISTER with
/// `421 Extension Required` (naming `sec-agree` in `Require`) or with
/// `494 Security Agreement Required`.
fn register_failure_demands_sec_agree(failure: &RegisterFailure) -> bool {
    if failure.auth_rounds != 0 {
        return false;
    }
    let Some(response) = failure.response.as_deref() else {
        return false;
    };
    match sip_frame::parse_status(response).ok() {
        // RFC 3329: a 494 means the security agreement is mandatory even when
        // the server does not echo a Require header, so it always escalates.
        Some(494) => return true,
        Some(421) => {}
        _ => return false,
    }
    sip_frame::header_values(response, "Require")
        .iter()
        .flat_map(|value| value.split(','))
        .any(|extension| extension.trim().eq_ignore_ascii_case("sec-agree"))
}

/// Read the `Min-Expires` floor from a `423 Interval Too Brief` response.
///
/// RFC 3261 §21.4.4: the registrar refuses a lease below this value. The
/// value is capped so a misconfigured floor cannot pin the registration open
/// forever.
fn parse_register_min_expires(response: &str) -> Option<u32> {
    let value = sip_frame::header_value(response.as_bytes(), "Min-Expires")?;
    let parsed = value.trim().parse::<u32>().ok()?;
    Some(parsed.min(MIN_EXPIRES_CAP).max(1))
}

fn classify_vowifi_register_error(reason: &str) -> RegistrationLossReason {
    if reason.starts_with("ims_aka_")
        || reason.starts_with("ims_digest_")
        || reason.starts_with("eap_aka_")
        || reason.starts_with("sim_auth_")
    {
        RegistrationLossReason::AuthenticationRejected
    } else if reason.contains("_timeout")
        || reason.contains("_read_")
        || reason.contains("_write_")
        || reason.contains("_connect_")
        || reason.contains("_bind_")
        || reason.contains("_shutdown_")
    {
        RegistrationLossReason::SignalingTransportLost
    } else {
        RegistrationLossReason::NetworkRejected
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_authenticated_register_exchange(
    line_id: &str,
    profile: &'static CarrierProfile,
    initial_channel: &mut SipChannel,
    gateway: &TunGatewayRuntime,
    context: &mut LiveRegisterRequestContext,
    challenge: &LiveDigestChallenge,
    variant: LiveRegisterHeaderVariant,
    authenticated_cseq: u32,
    resync_sent: &mut bool,
) -> Result<String, LiveStageError> {
    let mut auth_material =
        build_live_register_auth_material(line_id, profile, context, challenge, variant).await?;
    if let Some(auts) = auth_material.auts.take() {
        if *resync_sent {
            return Err(live_stage_error("ims_aka_resync_repeated"));
        }
        *resync_sent = true;
        let resync_authorization = build_digest_resync_authorization_header(
            context,
            challenge,
            &context.request_uri(profile, variant),
            &auts,
        )?;
        let resync_request = context.build_authorized_request(
            profile,
            variant,
            authenticated_cseq,
            &resync_authorization,
            None,
        );
        info!("IMS REGISTER AKA resync request ready");
        write_sip_request(initial_channel, &resync_request).await?;
        let response = read_final_register_response(initial_channel, &resync_request).await?;
        let summary = ims::parse_sip_response(&response, &context.target.realm)
            .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
        if !matches!(summary.status_code, 401 | 407) {
            return Err(live_stage_error("ims_aka_resync_unexpected_status"));
        }
        return Ok(response);
    }
    let selected_offer = select_live_security_server_offer(profile, challenge)?;
    let security_verify = selected_offer.as_ref().map(|offer| offer.raw.clone());
    if let Some(offer) =
        selected_offer.filter(|_| challenge.nonce_kind == LiveDigestNonceKind::AkaChallenge)
    {
        initial_channel
            .shutdown()
            .await
            .map_err(|_| live_stage_error("ims_sip_shutdown_failed"))?;
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
        let mut retry_cseq = authenticated_cseq;
        let mut expires = profile.ims.register.expires_seconds;
        let mut min_expires_rounds = 0u8;
        loop {
            let authenticated = context.build_register_request_with_expires(
                profile,
                variant,
                retry_cseq,
                Some(auth_material.authorization.as_str()),
                security_verify.as_deref(),
                expires,
            );
            write_sip_request(initial_channel, &authenticated).await?;
            let response = read_final_register_response(initial_channel, &authenticated).await?;
            let summary = ims::parse_sip_response(&response, &context.target.realm)
                .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
            if summary.status_code == 423 && min_expires_rounds < MAX_MIN_EXPIRES_ROUNDS {
                let Some(min_expires) = parse_register_min_expires(&response) else {
                    return Err(live_stage_error(
                        "ims_register_authenticated_min_expires_invalid",
                    ));
                };
                expires = expires.max(min_expires);
                retry_cseq = retry_cseq.saturating_add(1);
                min_expires_rounds += 1;
                info!(
                    sip_status = 423,
                    min_expires,
                    retry_cseq,
                    "VoWiFi authenticated REGISTER negotiating a longer lease"
                );
                continue;
            }
            return Ok(response);
        }
    }
}

// These protocol helpers keep security/session inputs explicit. Collapsing them
// into broad context bags would hide which values cross an authentication step.
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
        client_flow_local_port: local_security.port_c,
        client_flow_remote_port: offer.port_s,
        server_flow_local_port: local_security.port_s,
        server_flow_remote_port: offer.port_c,
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: true,
        udp_encapsulate: false,
    });
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_inverted",
        client_flow_local_port: local_security.port_c,
        client_flow_remote_port: offer.port_s,
        server_flow_local_port: local_security.port_s,
        server_flow_remote_port: offer.port_c,
        client_flow_outbound_sa_identifier: offer.spi_c,
        client_flow_inbound_sa_identifier: local_security.spi_s,
        server_flow_outbound_sa_identifier: offer.spi_s,
        server_flow_inbound_sa_identifier: local_security.spi_c,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: true,
        udp_encapsulate: false,
    });
    // TS 33.203 §7.3.2: the SA table pair is either (port_uc, port_ps) or
    // (port_us, port_pc). Some P-CSCFs register the UE->P-CSCF SA against the
    // second pair, so the protected REGISTER must then be sourced from port_us
    // towards port_pc with SPI = spi_pc. Try both port pairings and both SPI
    // assignments before giving up.
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_secondary_ports",
        client_flow_local_port: local_security.port_s,
        client_flow_remote_port: offer.port_c,
        server_flow_local_port: local_security.port_c,
        server_flow_remote_port: offer.port_s,
        client_flow_outbound_sa_identifier: offer.spi_c,
        client_flow_inbound_sa_identifier: local_security.spi_s,
        server_flow_outbound_sa_identifier: offer.spi_s,
        server_flow_inbound_sa_identifier: local_security.spi_c,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: true,
        udp_encapsulate: false,
    });
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_secondary_ports_inverted_spi",
        client_flow_local_port: local_security.port_s,
        client_flow_remote_port: offer.port_c,
        server_flow_local_port: local_security.port_c,
        server_flow_remote_port: offer.port_s,
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: true,
        udp_encapsulate: false,
    });
    for alt in &auth_material.ims_esp_alt_secrets {
        candidates.push(LiveImsEspPolicyCandidate {
            label: "client_server_flow_primary_raw_ik",
            client_flow_local_port: local_security.port_c,
            client_flow_remote_port: offer.port_s,
            server_flow_local_port: local_security.port_s,
            server_flow_remote_port: offer.port_c,
            client_flow_outbound_sa_identifier: offer.spi_s,
            client_flow_inbound_sa_identifier: local_security.spi_c,
            server_flow_outbound_sa_identifier: offer.spi_c,
            server_flow_inbound_sa_identifier: local_security.spi_s,
            secrets: alt.clone(),
            icv_include_iv: true,
            udp_encapsulate: false,
        });
    }
    // Interop probes for P-CSCFs that deviate from RFC 4303 / raw ESP:
    // 1) ICV computed without the explicit IV;
    // 2) ESP carried inside a UDP header (RFC 3948) on the protected ports;
    // 3) both deviations at once.
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_primary_icv_excludes_iv",
        client_flow_local_port: local_security.port_c,
        client_flow_remote_port: offer.port_s,
        server_flow_local_port: local_security.port_s,
        server_flow_remote_port: offer.port_c,
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: false,
        udp_encapsulate: false,
    });
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_primary_udp_encap",
        client_flow_local_port: local_security.port_c,
        client_flow_remote_port: offer.port_s,
        server_flow_local_port: local_security.port_s,
        server_flow_remote_port: offer.port_c,
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: true,
        udp_encapsulate: true,
    });
    candidates.push(LiveImsEspPolicyCandidate {
        label: "client_server_flow_primary_udp_encap_icv_excludes_iv",
        client_flow_local_port: local_security.port_c,
        client_flow_remote_port: offer.port_s,
        server_flow_local_port: local_security.port_s,
        server_flow_remote_port: offer.port_c,
        client_flow_outbound_sa_identifier: offer.spi_s,
        client_flow_inbound_sa_identifier: local_security.spi_c,
        server_flow_outbound_sa_identifier: offer.spi_c,
        server_flow_inbound_sa_identifier: local_security.spi_s,
        secrets: auth_material.ims_esp_secrets.clone(),
        icv_include_iv: false,
        udp_encapsulate: true,
    });
    if let Some(null_secrets) = &auth_material.ims_esp_null_secrets {
        candidates.push(LiveImsEspPolicyCandidate {
            label: "client_server_flow_primary_null_encryption",
            client_flow_local_port: local_security.port_c,
            client_flow_remote_port: offer.port_s,
            server_flow_local_port: local_security.port_s,
            server_flow_remote_port: offer.port_c,
            client_flow_outbound_sa_identifier: offer.spi_s,
            client_flow_inbound_sa_identifier: local_security.spi_c,
            server_flow_outbound_sa_identifier: offer.spi_c,
            server_flow_inbound_sa_identifier: local_security.spi_s,
            secrets: null_secrets.clone(),
            icv_include_iv: true,
            udp_encapsulate: false,
        });
        candidates.push(LiveImsEspPolicyCandidate {
            label: "client_server_flow_primary_null_encryption_udp_encap",
            client_flow_local_port: local_security.port_c,
            client_flow_remote_port: offer.port_s,
            server_flow_local_port: local_security.port_s,
            server_flow_remote_port: offer.port_c,
            client_flow_outbound_sa_identifier: offer.spi_s,
            client_flow_inbound_sa_identifier: local_security.spi_c,
            server_flow_outbound_sa_identifier: offer.spi_c,
            server_flow_inbound_sa_identifier: local_security.spi_s,
            secrets: null_secrets.clone(),
            icv_include_iv: true,
            udp_encapsulate: true,
        });
    }

    let mut last_error = None;
    for (candidate_index, candidate) in candidates.iter().enumerate() {
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
                    local_port: candidate.client_flow_local_port,
                    remote_port: candidate.client_flow_remote_port,
                    outbound_sa_identifier: candidate.client_flow_outbound_sa_identifier,
                    inbound_sa_identifier: candidate.client_flow_inbound_sa_identifier,
                    secrets: candidate.secrets.clone(),
                    icv_include_iv: candidate.icv_include_iv,
                    udp_encapsulate: candidate.udp_encapsulate,
                },
                server_flow: ImsEspFlowConfig {
                    label: "server_flow",
                    local_port: candidate.server_flow_local_port,
                    remote_port: candidate.server_flow_remote_port,
                    outbound_sa_identifier: candidate.server_flow_outbound_sa_identifier,
                    inbound_sa_identifier: candidate.server_flow_inbound_sa_identifier,
                    secrets: candidate.secrets.clone(),
                    icv_include_iv: candidate.icv_include_iv,
                    udp_encapsulate: candidate.udp_encapsulate,
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
            client_flow_outbound_spi = candidate.client_flow_outbound_sa_identifier,
            client_flow_inbound_spi = candidate.client_flow_inbound_sa_identifier,
            candidate_index,
            "IMS REGISTER will continue over protected ipsec-3gpp transport"
        );
        let target = SocketAddr::new(context.route_addr, candidate.client_flow_remote_port);
        let transport = ims_transport(profile);
        let ue_socket = ue_socket_context_for_line(line_id);
        match connect_sip_socket(
            gateway.inner_addr(),
            target,
            candidate.client_flow_local_port,
            transport,
            Some(gateway.tun_name()),
            ue_socket.as_ref(),
        )
        .await
        {
            Ok(protected_socket) => {
                let protected_local_addr = protected_socket
                    .local_addr()
                    .map_err(|_| live_stage_error("ims_sip_local_addr_unavailable"))?;
                context.local_addr = protected_local_addr;
                let protected_route = crate::connectivity::core::context::ImsRoute {
                    local_addr: protected_local_addr,
                    pcscf_addr: target,
                    transport,
                };
                // TS 33.203 §7.1 UDP: responses arrive on the UE's protected
                // server port (port_us) from the P-CSCF's protected client port
                // (port_pc) -- a different socket than the one that sent the
                // REGISTER (port_uc -> port_ps). Bind a dedicated listener so
                // the 200 OK is not dropped by the kernel for lack of a socket.
                // The secondary-pair candidates already source from port_us and
                // read the response on the same connected socket.
                let primary_pairing = matches!(
                    transport,
                    crate::connectivity::core::context::SipTransport::Udp
                ) && candidate.client_flow_local_port
                    == local_security.port_c;
                let mut protected_channel = if primary_pairing {
                    match protected_socket {
                        SipChannelSocket::Udp(send_socket) => {
                            match connect_sip_socket(
                                gateway.inner_addr(),
                                SocketAddr::new(context.route_addr, offer.port_c),
                                local_security.port_s,
                                transport,
                                Some(gateway.tun_name()),
                                ue_socket.as_ref(),
                            )
                            .await
                            {
                                Ok(SipChannelSocket::Udp(receive_socket)) => {
                                    SipChannel::new_udp_pair(
                                        send_socket,
                                        receive_socket,
                                        Vec::new(),
                                        protected_route,
                                        security_verify.map(str::to_string),
                                    )
                                }
                                _ => SipChannel::new(
                                    SipChannelSocket::Udp(send_socket),
                                    Vec::new(),
                                    protected_route,
                                    security_verify.map(str::to_string),
                                ),
                            }
                        }
                        other => SipChannel::new(
                            other,
                            Vec::new(),
                            protected_route,
                            security_verify.map(str::to_string),
                        ),
                    }
                } else {
                    SipChannel::new(
                        protected_socket,
                        Vec::new(),
                        protected_route,
                        security_verify.map(str::to_string),
                    )
                };
                // TS 24.229 §5.1.1.2.2 b/c: a UDP REGISTER protected by a
                // security association advertises the protected server port
                // (port_us) in Via and Contact, even though the packet itself
                // is sourced from the protected client port (port_uc).
                let protected_header_port = match transport {
                    crate::connectivity::core::context::SipTransport::Udp => {
                        Some(local_security.port_s)
                    }
                    crate::connectivity::core::context::SipTransport::Tcp => None,
                };
                context.protected_header_port = protected_header_port;
                let authenticated = context.build_authorized_request(
                    profile,
                    variant,
                    authenticated_cseq,
                    &auth_material.authorization,
                    security_verify,
                );
                info!(
                    policy_candidate = candidate.label,
                    authenticated_cseq,
                    via_port = protected_header_port.unwrap_or(profile.ims.local_port),
                    contact_port = protected_header_port.unwrap_or(profile.ims.local_port),
                    protected_socket_local = protected_local_addr.to_string().as_str(),
                    protected_target = target.to_string().as_str(),
                    authorization_present = true,
                    security_verify_present = security_verify.is_some(),
                    security_client_present = true,
                    transport = transport.as_via(),
                    "IMS REGISTER protected request headers prepared"
                );
                write_sip_request(&mut protected_channel, &authenticated).await?;
                // The primary candidate gets the full transaction budget; an
                // unanswered alternate candidate is likely the same silent
                // IPsec drop, so probe it with a shorter window before moving
                // to the next SPI/key mapping.
                let candidate_timeout = if candidate_index == 0 {
                    LIVE_IMS_REGISTER_READ_TIMEOUT
                } else {
                    LIVE_IMS_REGISTER_CANDIDATE_READ_TIMEOUT
                };
                let mut response = match read_final_register_response_with_timeout(
                    &mut protected_channel,
                    &authenticated,
                    candidate_timeout,
                )
                .await
                {
                    Ok(response) => response,
                    Err(err) => {
                        protected_channel.abort();
                        warn!(
                            policy_candidate = candidate.label,
                            candidate_index,
                            reason = err.reason.as_str(),
                            "IMS protected SIP candidate timed out; trying next policy"
                        );
                        last_error = Some(err);
                        continue;
                    }
                };
                let mut summary = ims::parse_sip_response(&response, &context.target.realm)
                    .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
                let mut retry_cseq = authenticated_cseq;
                let mut expires = profile.ims.register.expires_seconds;
                let mut min_expires_rounds = 0u8;
                loop {
                    if summary.status_code == 200 {
                        let artifacts = RegisterArtifacts::parse(response.as_bytes());
                        let registered = RegisteredImsContext::from_artifacts(
                            ImsRegistrationAccess::Vowifi,
                            artifacts,
                            expires,
                        );
                        let mut registered_identity = context.identity.shared.clone();
                        if let Some(uri) = registered.default_associated_uri() {
                            registered_identity.public_uri = uri.to_string();
                        }
                        // The registrar's P-Associated-URI set is the only place a
                        // data-only line's own MSISDN is observable, and this leg
                        // previously only logged how many URIs arrived. Publish the
                        // telephone identities so the API layer can surface the
                        // number instead of reporting N/A.
                        crate::connectivity::core::own_numbers::record(
                            line_id,
                            crate::connectivity::core::ims_failure::telephone_numbers_from_register_success(
                                response.as_bytes(),
                            ),
                        );
                        record_live_ims_security_verify(
                            line_id,
                            profile,
                            security_verify,
                            &registered,
                        )
                        .await;
                        record_live_ims_channel(
                            line_id,
                            profile,
                            registered_identity,
                            protected_channel,
                            security_verify.map(str::to_string),
                            registered,
                            context.clone(),
                            variant,
                            retry_cseq.saturating_add(1),
                        )
                        .await;
                        return Ok(response);
                    }
                    if summary.status_code != 423 || min_expires_rounds >= MAX_MIN_EXPIRES_ROUNDS {
                        protected_channel.abort();
                        return Ok(response);
                    }
                    let Some(min_expires) = parse_register_min_expires(&response) else {
                        protected_channel.abort();
                        last_error = Some(live_stage_error(
                            "ims_register_authenticated_min_expires_invalid",
                        ));
                        break;
                    };
                    expires = expires.max(min_expires);
                    retry_cseq = retry_cseq.saturating_add(1);
                    min_expires_rounds += 1;
                    info!(
                        policy_candidate = candidate.label,
                        sip_status = 423,
                        min_expires,
                        retry_cseq,
                        "IMS REGISTER protected 423 negotiation"
                    );
                    let authenticated = context.build_register_request_with_expires(
                        profile,
                        variant,
                        retry_cseq,
                        Some(auth_material.authorization.as_str()),
                        security_verify,
                        expires,
                    );
                    write_sip_request(&mut protected_channel, &authenticated).await?;
                    response = match read_final_register_response_with_timeout(
                        &mut protected_channel,
                        &authenticated,
                        candidate_timeout,
                    )
                    .await
                    {
                        Ok(response) => response,
                        Err(err) => {
                            protected_channel.abort();
                            warn!(
                                policy_candidate = candidate.label,
                                candidate_index,
                                reason = err.reason.as_str(),
                                "IMS protected 423 retry timed out; trying next policy"
                            );
                            last_error = Some(err);
                            break;
                        }
                    };
                    summary = ims::parse_sip_response(&response, &context.target.realm)
                        .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
                }
            }
            Err(err) => {
                warn!(
                    policy_candidate = candidate.label,
                    candidate_index,
                    reason = err.reason.as_str(),
                    "IMS protected SIP candidate failed"
                );
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| live_stage_error("ims_sip_connect_failed")))
}

struct LiveImsEspPolicyCandidate {
    label: &'static str,
    client_flow_local_port: u16,
    client_flow_remote_port: u16,
    server_flow_local_port: u16,
    server_flow_remote_port: u16,
    client_flow_outbound_sa_identifier: u32,
    client_flow_inbound_sa_identifier: u32,
    server_flow_outbound_sa_identifier: u32,
    server_flow_inbound_sa_identifier: u32,
    secrets: ChildSaSecretPair,
    /// RFC 4303 covers the explicit IV in the ICV; false probes the
    /// non-standard convention that skips the IV.
    icv_include_iv: bool,
    /// True wraps the ESP frame in a UDP header (RFC 3948) rather than raw
    /// ESP (IP protocol 50) inside the tunnel.
    udp_encapsulate: bool,
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
    let service_route = cached_live_ims_registration(line_id, profile)
        .await
        .and_then(|registration| registration.service_route);
    if let Some(outcome) = send_live_sms_message_on_cached_channel(
        line_id,
        profile,
        route,
        identity,
        submission,
        variant,
        service_route.as_deref(),
        security_verify,
    )
    .await?
    {
        return Ok(outcome);
    }

    let target = SocketAddr::new(route.remote_addr, route.remote_port);
    let transport = ims_transport(profile);
    let tun_name = tun_name_for_line(&live_runtime_config().tun_name, line_id);
    let ue_socket = ue_socket_context_for_line(line_id);
    let socket = connect_sip_socket(
        route.local_addr,
        target,
        route.local_port,
        transport,
        Some(&tun_name),
        ue_socket.as_ref(),
    )
    .await?;
    let mut pending = Vec::new();
    let local_addr = socket
        .local_addr()
        .map_err(|_| live_stage_error("sms_sip_local_addr_unavailable"))?;
    let mut channel = SipChannel::new(
        socket,
        Vec::new(),
        shared_vowifi_route(profile, route, local_addr),
        security_verify.map(str::to_string),
    );
    match send_live_sms_message_on_stream(
        profile,
        route,
        identity,
        submission,
        variant,
        service_route.as_deref(),
        security_verify,
        local_addr,
        &mut channel,
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
            service_route.clone(),
            security_verify.map(ToString::to_string),
            local_addr,
            channel,
            pending,
            outcome,
        )),
        Err(err) => {
            channel.abort();
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
    service_route: Option<&str>,
    security_verify: Option<&str>,
) -> Result<Option<LiveSmsSendResult>, LiveStageError> {
    let channel = {
        let mut guard = ims_channel_cache().lock().await;
        let Some(channel) = guard.remove(line_id) else {
            return Ok(None);
        };
        channel
    };
    if channel.profile_id != profile.meta.profile_id || channel.expires_at <= Instant::now() {
        channel.channel.abort();
        return Ok(None);
    }
    let local_addr = channel.channel.route().local_addr;
    let (socket, mut pending) = channel.channel.into_parts();
    let mut channel = SipChannel::new(
        socket,
        Vec::new(),
        shared_vowifi_route(profile, route, local_addr),
        security_verify.map(str::to_string),
    );
    match send_live_sms_message_on_stream(
        profile,
        route,
        identity,
        submission,
        variant,
        service_route,
        security_verify,
        local_addr,
        &mut channel,
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
            service_route.map(ToString::to_string),
            security_verify.map(ToString::to_string),
            local_addr,
            channel,
            pending,
            outcome,
        ))),
        Err(err) => {
            warn!(
                profile_id = profile.meta.profile_id,
                reason = err.reason.as_str(),
                "VoWiFi cached IMS channel failed during MESSAGE exchange"
            );
            channel.abort();
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
    service_route: Option<&str>,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
    channel: &mut SipChannel,
    pending: &mut Vec<u8>,
) -> Result<sms::MoSmsSipOutcome, LiveStageError> {
    let request = build_live_sms_message_request(
        profile,
        route,
        identity,
        submission,
        variant,
        service_route,
        security_verify,
        local_addr,
    );
    write_sip_frame(channel, &request).await?;
    let response_frame = read_sip_frame_buffered(
        channel,
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
    service_route: Option<String>,
    security_verify: Option<String>,
    local_addr: SocketAddr,
    mut channel: SipChannel,
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
            &mut channel,
            &submission,
            &variant,
            service_route.as_deref(),
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
                let (socket, channel_pending) = channel.into_parts();
                let mut merged_pending = channel_pending;
                merged_pending.extend_from_slice(&pending);
                let mut guard = ims_channel_cache().lock().await;
                guard.insert(
                    line_id.clone(),
                    LiveImsChannel {
                        profile_id: profile.meta.profile_id,
                        expires_at,
                        channel: SipChannel::new(
                            socket,
                            merged_pending,
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
                    "VoWiFi SMS follow-up task failed; IMS channel discarded"
                );
                channel.abort();
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
    channel: &mut SipChannel,
    submission: &sms::MoSmsSubmission,
    variant: &LiveSmsRequestUriVariant,
    service_route: Option<&str>,
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
        match read_sip_frame_buffered(channel, pending, timeout, "sms_message_ack_timeout").await {
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
                write_sip_frame(channel, &response).await?;
                if let Some(deliver) = mt_deliver {
                    let rp_ack_body = sms::build_network_rp_ack(deliver.rp_message_reference);
                    let rp_ack_request = build_live_sms_rp_ack_request(
                        profile,
                        route,
                        identity,
                        variant,
                        &frame,
                        &rp_ack_body,
                        service_route,
                        security_verify,
                        local_addr,
                    );
                    write_sip_frame(channel, &rp_ack_request).await?;
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
    service_route: Option<&str>,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let to_value = format!("<{}>", variant.to_uri);
    let mut headers = vec![
        crate::connectivity::core::sip_message::SipHeader::new(
            "Route",
            service_route.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "<sip:{}:{};lr>",
                    sip_host(route.remote_addr),
                    profile.ims.local_port
                )
            }),
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
    crate::connectivity::core::sip_message::build_message(
        &crate::connectivity::core::sip_message::SipRequest {
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
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn build_live_sms_rp_ack_request(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    identity: &LiveImsRegisterIdentity,
    variant: &LiveSmsRequestUriVariant,
    inbound_frame: &[u8],
    body: &[u8],
    service_route: Option<&str>,
    security_verify: Option<&str>,
    local_addr: SocketAddr,
) -> Vec<u8> {
    let branch = format!("z9hG4bK{}", hex_token(12));
    let call_id = format!("{}@simadmin", hex_token(16));
    let from_tag = hex_token(8);
    let request_uri =
        sip_header_uri(inbound_frame, "From").unwrap_or_else(|| variant.request_uri.clone());
    let to_value = format!("<{request_uri}>");
    let mut headers = vec![
        crate::connectivity::core::sip_message::SipHeader::new(
            "Route",
            service_route.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "<sip:{}:{};lr>",
                    sip_host(route.remote_addr),
                    profile.ims.local_port
                )
            }),
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
    crate::connectivity::core::sip_message::build_rp_ack(
        &crate::connectivity::core::sip_message::SipRequest {
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
        },
    )
}

fn shared_vowifi_route(
    profile: &'static CarrierProfile,
    route: &tun_gateway::ImsClientTcpRoute,
    local_addr: SocketAddr,
) -> crate::connectivity::core::context::ImsRoute {
    crate::connectivity::core::context::ImsRoute {
        local_addr: SocketAddr::new(local_addr.ip(), profile.ims.local_port),
        pcscf_addr: SocketAddr::new(route.remote_addr, profile.ims.local_port),
        transport: ims_transport(profile),
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
    crate::connectivity::core::sip_message::build_invite(
        &crate::connectivity::core::sip_message::SipRequest {
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
        },
    )
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
    crate::connectivity::core::sip_message::build_ack(
        &crate::connectivity::core::sip_message::SipRequest {
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
        },
    )
}

fn sip_header_uri(frame: &[u8], header_name: &str) -> Option<String> {
    let header_end = find_sip_header_end(frame)?;
    let headers = std::str::from_utf8(&frame[..header_end]).ok()?;
    sip_header_values(headers, header_name)
        .into_iter()
        .find_map(|value| sip_uri_from_header_value(&value))
}

fn sip_dialog_tag(frame: &[u8], header_name: &str) -> Option<String> {
    crate::connectivity::core::sip_frame::header_value(frame, header_name)?
        .split(';')
        .skip(1)
        .find_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("tag")
                .then(|| value.trim().trim_matches('"').to_string())
        })
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
    line_id: &str,
    profile: &'static CarrierProfile,
    recipient: &str,
    service_center: &str,
) -> Result<Vec<LiveSmsRequestUriVariant>, LiveStageError> {
    let recipient_user = sip_phone_user(recipient)?;
    let service_center_user = sip_phone_user(service_center)?;
    let domain = live_ims_target(line_id, profile).domain;
    let to_uri = format!("sip:{recipient_user}@{domain};user=phone");
    Ok(vec![
        LiveSmsRequestUriVariant {
            label: "service_center_sip_user_phone",
            request_uri: format!("sip:{service_center_user}@{domain};user=phone"),
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

async fn write_sip_request(channel: &mut SipChannel, request: &str) -> Result<(), LiveStageError> {
    channel
        .send_all(request.as_bytes())
        .await
        .map_err(|_| live_stage_error("ims_register_write_failed"))
}

async fn write_sip_frame(channel: &mut SipChannel, frame: &[u8]) -> Result<(), LiveStageError> {
    channel
        .send_all(frame)
        .await
        .map_err(|_| live_stage_error("sms_message_write_failed"))
}

async fn connect_sip_socket(
    inner_addr: IpAddr,
    target: SocketAddr,
    preferred_local_port: u16,
    transport: crate::connectivity::core::context::SipTransport,
    interface: Option<&str>,
    ue_socket: Option<&LiveUeSocketContext>,
) -> Result<SipChannelSocket, LiveStageError> {
    if let Some(context) = ue_socket {
        let local = SocketAddr::new(inner_addr, preferred_local_port);
        let device = interface.map(str::to_string);
        return match transport {
            crate::connectivity::core::context::SipTransport::Tcp => {
                let spec = UeSocketSpec::tcp_connected(
                    local,
                    target,
                    device,
                    LIVE_IMS_TCP_TIMEOUT.as_secs().max(1),
                );
                match context.worker.create_socket(spec).await {
                    Ok(UeSocket::Tcp(stream)) => Ok(SipChannelSocket::Tcp(stream)),
                    Ok(_) => Err(live_stage_error("ims_ue_socket_family_mismatch")),
                    Err(error) => {
                        warn!(
                            line_id = %context.namespace,
                            error = %error,
                            "UE worker SIP TCP socket creation failed"
                        );
                        Err(live_stage_error("ims_ue_tcp_socket_creation_failed"))
                    }
                }
            }
            crate::connectivity::core::context::SipTransport::Udp => {
                let spec = UeSocketSpec::udp_connected(local, target, device);
                match context.worker.create_socket(spec).await {
                    Ok(UeSocket::Udp(socket)) => Ok(SipChannelSocket::Udp(socket)),
                    Ok(_) => Err(live_stage_error("ims_ue_socket_family_mismatch")),
                    Err(error) => {
                        warn!(
                            line_id = %context.namespace,
                            error = %error,
                            "UE worker SIP UDP socket creation failed"
                        );
                        Err(live_stage_error("ims_ue_udp_socket_creation_failed"))
                    }
                }
            }
        };
    }
    match transport {
        crate::connectivity::core::context::SipTransport::Tcp => {
            let socket = match target {
                SocketAddr::V4(_) => TcpSocket::new_v4(),
                SocketAddr::V6(_) => TcpSocket::new_v6(),
            }
            .map_err(|_| live_stage_error("ims_tcp_socket_failed"))?;
            bind_socket_to_interface(&socket, interface)
                .map_err(|_| live_stage_error("ims_tcp_bind_interface_failed"))?;
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
                .map(SipChannelSocket::Tcp)
        }
        crate::connectivity::core::context::SipTransport::Udp => {
            let local_port = if preferred_local_port != 0 {
                preferred_local_port
            } else {
                0
            };
            let socket =
                bind_udp_socket_to_interface(SocketAddr::new(inner_addr, local_port), interface)
                    .await
                    .map_err(|_| live_stage_error("ims_udp_bind_failed"))?;
            socket
                .connect(target)
                .await
                .map_err(|_| live_stage_error("ims_udp_connect_failed"))?;
            Ok(SipChannelSocket::Udp(socket))
        }
    }
}

async fn bind_udp_socket_to_interface(
    local: SocketAddr,
    interface: Option<&str>,
) -> std::io::Result<tokio::net::UdpSocket> {
    let Some(interface) = interface.filter(|value| !value.trim().is_empty()) else {
        return tokio::net::UdpSocket::bind(local).await;
    };
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(local),
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    bind_raw_socket_to_interface(&socket, interface)?;
    socket.bind(&local.into())?;
    socket.set_nonblocking(true)?;
    tokio::net::UdpSocket::from_std(socket.into())
}

fn bind_socket_to_interface(
    _socket: &tokio::net::TcpSocket,
    interface: Option<&str>,
) -> std::io::Result<()> {
    let Some(interface) = interface.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::fd::AsRawFd};
        let name = CString::new(interface).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "interface contains NUL")
        })?;
        let result = unsafe {
            libc::setsockopt(
                _socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                name.as_ptr().cast(),
                name.as_bytes_with_nul().len() as libc::socklen_t,
            )
        };
        if result == 0 {
            return Ok(());
        }
        return Err(std::io::Error::last_os_error());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = interface;
        Ok(())
    }
}

fn bind_raw_socket_to_interface(socket: &socket2::Socket, interface: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::fd::AsRawFd};
        let name = CString::new(interface).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "interface contains NUL")
        })?;
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
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (socket, interface);
        Ok(())
    }
}

fn ims_transport(
    profile: &'static CarrierProfile,
) -> crate::connectivity::core::context::SipTransport {
    match profile.ims.transport {
        "tcp" => crate::connectivity::core::context::SipTransport::Tcp,
        _ => crate::connectivity::core::context::SipTransport::Udp,
    }
}

async fn read_sip_response(channel: &mut SipChannel) -> Result<String, LiveStageError> {
    read_sip_response_with_timeout(channel, LIVE_IMS_REGISTER_READ_TIMEOUT).await
}

async fn read_final_register_response(
    channel: &mut SipChannel,
    request: &str,
) -> Result<String, LiveStageError> {
    read_final_register_response_with_timeout(channel, request, LIVE_IMS_REGISTER_READ_TIMEOUT)
        .await
}

async fn read_final_register_response_with_timeout(
    channel: &mut SipChannel,
    request: &str,
    timeout: Duration,
) -> Result<String, LiveStageError> {
    let expected_transaction = RegisterTransactionKey::from_register_request(request.as_bytes());
    let deadline = tokio::time::Instant::now() + timeout;
    let mut provisional_count = 0u8;
    let mut ignored_frames = 0u8;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(live_stage_error("ims_register_read_timeout"));
        }
        let frame = channel
            .recv_sip_fresh(remaining)
            .await
            .map_err(|error| live_stage_error(error.code()))?;
        let is_response = frame.starts_with(b"SIP/2.0");
        let transaction_matches = expected_transaction
            .as_ref()
            .map(|expected| expected.matches_response(&frame))
            .unwrap_or(is_response);
        if !transaction_matches {
            ignored_frames += 1;
            channel.requeue(frame);
            if ignored_frames >= MAX_REGISTER_IGNORED_FRAMES {
                return Err(live_stage_error("ims_register_read_timeout"));
            }
            debug!(
                is_response,
                transaction_key_available = expected_transaction.is_some(),
                "VoWiFi adapter-owned REGISTER skipping unrelated SIP frame"
            );
            continue;
        }

        let status = sip_frame::parse_status(&frame)
            .map_err(|_| live_stage_error("ims_register_response_parse_failed"))?;
        if !(100..=199).contains(&status) {
            return String::from_utf8(frame)
                .map_err(|_| live_stage_error("ims_register_response_not_utf8"));
        }
        provisional_count += 1;
        if provisional_count >= MAX_REGISTER_PROVISIONAL_RESPONSES {
            return Err(live_stage_error(
                "ims_register_authenticated_unexpected_status",
            ));
        }
        debug!(
            sip_status = status,
            provisional_count, "VoWiFi adapter-owned REGISTER provisional response received"
        );
    }
}

async fn read_sip_response_with_timeout(
    channel: &mut SipChannel,
    timeout: Duration,
) -> Result<String, LiveStageError> {
    let buffer = read_sip_frame(channel, timeout, "ims_register_read_timeout").await?;
    String::from_utf8(buffer).map_err(|_| live_stage_error("ims_register_response_not_utf8"))
}

async fn read_sip_frame(
    channel: &mut SipChannel,
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
            read = channel.recv_chunk(&mut chunk) => {
                let read = read.map_err(|_| live_stage_error("ims_register_read_failed"))?;
                if read == 0 && channel.is_tcp() {
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
    channel: &mut SipChannel,
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
            read = channel.recv_chunk(&mut chunk) => {
                let read = read.map_err(|_| live_stage_error("ims_register_read_failed"))?;
                if read == 0 && channel.is_tcp() {
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
    crate::connectivity::core::sip_frame::parse_status(frame)
        .map_err(|err| live_stage_error(err.code()))
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

#[derive(Clone)]
struct LiveRegisterRequestContext {
    identity: LiveImsRegisterIdentity,
    target: LiveImsTarget,
    local_addr: SocketAddr,
    route_addr: IpAddr,
    transport: crate::connectivity::core::context::SipTransport,
    from_tag: String,
    call_id: String,
    instance_id: String,
    security_client_state: LiveSecurityClientState,
    security_client_full_spaced: String,
    security_client_full_compact: String,
    security_client_minimal_spaced: String,
    /// Port advertised in Via/Contact of a protected REGISTER. For UDP,
    /// TS 24.229 §5.1.1.2.2 requires the protected server port (port_us) in
    /// both headers even though the request is sourced from port_uc. None means
    /// the round is unprotected and headers use the normal SIP port.
    protected_header_port: Option<u16>,
    /// Real serving-cell snapshot used only for CNI while the SIP access itself
    /// remains IWLAN. It is fixed for initial/authenticated/refresh/unregister.
    access_network: Option<ImsAccessNetworkContext>,
    video_capability_enabled: bool,
}

impl LiveRegisterRequestContext {
    fn new(
        profile: &'static CarrierProfile,
        identity: LiveImsRegisterIdentity,
        local_addr: SocketAddr,
        route_addr: IpAddr,
    ) -> Result<Self, LiveStageError> {
        Self::new_with_target(
            profile,
            live_ims_target("", profile),
            identity,
            local_addr,
            route_addr,
        )
    }

    fn new_for_line(
        line_id: &str,
        profile: &'static CarrierProfile,
        identity: LiveImsRegisterIdentity,
        local_addr: SocketAddr,
        route_addr: IpAddr,
    ) -> Result<Self, LiveStageError> {
        let device_imei = line_overrides(line_id).effective_device_imei;
        Self::new_with_target_and_device(
            profile,
            live_ims_target(line_id, profile),
            identity,
            local_addr,
            route_addr,
            device_imei.as_deref(),
            super::operator::operator_link_for_line(line_id).video_enabled(),
        )
    }

    fn new_with_target(
        profile: &'static CarrierProfile,
        target: LiveImsTarget,
        identity: LiveImsRegisterIdentity,
        local_addr: SocketAddr,
        route_addr: IpAddr,
    ) -> Result<Self, LiveStageError> {
        Self::new_with_target_and_device(
            profile, target, identity, local_addr, route_addr, None, false,
        )
    }

    fn new_with_target_and_device(
        profile: &'static CarrierProfile,
        target: LiveImsTarget,
        identity: LiveImsRegisterIdentity,
        local_addr: SocketAddr,
        route_addr: IpAddr,
        device_imei: Option<&str>,
        video_capability_enabled: bool,
    ) -> Result<Self, LiveStageError> {
        let security_client_state = LiveSecurityClientState::new(live_runtime_config())?;
        let instance_id = format_sip_instance_id(profile, &identity, device_imei);
        Ok(Self {
            identity,
            target,
            local_addr,
            route_addr,
            transport: ims_transport(profile),
            from_tag: hex_token(8),
            call_id: format!("{}@simadmin", hex_token(16)),
            instance_id,
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
            protected_header_port: None,
            access_network: None,
            video_capability_enabled,
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
        self.build_register_request_with_expires(
            profile,
            variant,
            cseq,
            authorization,
            security_verify,
            profile.ims.register.expires_seconds,
        )
    }

    fn build_unregister_request(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
        cseq: u32,
        authorization: Option<&str>,
        security_verify: Option<&str>,
    ) -> String {
        self.build_register_request_with_expires(
            profile,
            variant,
            cseq,
            authorization,
            security_verify,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_register_request_with_expires(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
        cseq: u32,
        authorization: Option<&str>,
        security_verify: Option<&str>,
        expires_seconds: u32,
    ) -> String {
        let branch = format!("z9hG4bK{}", hex_token(12));
        let request_uri = self.request_uri(profile, variant);
        let local_host = sip_host(self.local_addr.ip());
        let require_sec_agree = sec_agree_headers_required(
            profile,
            variant.force_sec_agree_headers,
            variant.suppress_sec_agree_headers,
        );
        let phase_includes_pani = if cseq == 1 {
            profile.ims.register.include_pani_initial
        } else {
            profile.ims.register.include_pani_authenticated
        };
        let pani = match variant.header_profile.pani {
            LivePaniFormat::ProfileDefault => phase_includes_pani
                .then(|| {
                    resolve_access_identity(
                        profile.ims.register.pani_identity_policy,
                        Some(profile.ims.register.access_network_info),
                        None,
                    )
                    .value
                })
                .flatten(),
            LivePaniFormat::PlainWifi => Some("IEEE-802.11".to_string()),
            LivePaniFormat::Omit => None,
        };
        #[allow(unused_mut)]
        let mut visited_network_header = profile
            .ims
            .register
            .visited_network_header
            .map(str::to_string);
        #[cfg(test)]
        if visited_network_header.is_none() {
            let visited_network = format!(
                "ims.mnc{}.mcc{}.3gppnetwork.org",
                three_digit_mnc(profile),
                profile.meta.mcc
            );
            visited_network_header = match variant.header_profile.visited_network {
                LiveVisitedNetworkFormat::QuotedHome => Some(format!("\"{visited_network}\"")),
                LiveVisitedNetworkFormat::UnquotedHome => Some(visited_network),
                LiveVisitedNetworkFormat::Omit => None,
            };
        }
        let params = crate::connectivity::core::context::ImsRegisterParams {
            realm: self.target.realm.clone(),
            domain: self.target.domain.clone(),
            registrar: self.target.registrar.clone(),
            supported_header: profile.ims.register.supported_header.to_string(),
            require_sec_agree,
            user_agent: build_live_user_agent(profile, variant.header_profile.user_agent),
            pani,
            visited_network: visited_network_header,
            allow_header: profile
                .ims
                .register
                .allow_methods
                .unwrap_or_default()
                .to_string(),
            // Carrier-configurable: some networks reject the common 3600 default.
            expires: expires_seconds,
        };
        let authorization = authorization.map(str::to_string).or_else(|| {
            (cseq == 1)
                .then(|| self.build_initial_authorization_header(profile, variant))
                .flatten()
        });
        let contact = self.build_contact_header(profile, &local_host, variant.header_profile);
        let contact = contact
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("Contact: ")
            .unwrap_or(contact.as_str())
            .to_string();
        // TS 24.229 only permits Cellular-Network-Info in messages where the
        // access-network information is also present. Keep this phase-aware:
        // a profile may intentionally omit both headers on the initial request
        // and add them only after authentication.
        let cellular_network_info = (params.pani.is_some()
            && profile.ims.register.enable_cellular_network_info
            && variant.header_profile.include_cellular_network_info)
            .then(|| {
                resolve_access_identity(
                    profile.ims.register.cni_identity_policy,
                    profile.ims.register.cellular_network_info,
                    self.access_network.as_ref(),
                )
                .value
            })
            .flatten();
        // A security-protected authenticated round repeats Security-Client only
        // when this candidate advertised it or the response supplied an actual
        // Security-Server offer (represented by Security-Verify). A plain AKA
        // 401 must not silently turn an `auto`/`disabled` profile into sec-agree.
        let to_value = format!("<{}>", self.identity.public_uri);
        let frame = crate::connectivity::core::register_message::build_register(
            &crate::connectivity::core::register_message::RegisterRequest {
                request_uri,
                advertised_route: crate::connectivity::core::context::ImsRoute {
                    local_addr: SocketAddr::new(
                        self.local_addr.ip(),
                        self.protected_header_port.unwrap_or(profile.ims.local_port),
                    ),
                    pcscf_addr: SocketAddr::new(self.route_addr, profile.ims.local_port),
                    transport: self.transport,
                },
                branch,
                from_uri: self.identity.public_uri.clone(),
                from_tag: self.from_tag.clone(),
                to_value,
                call_id: self.call_id.clone(),
                cseq,
                headers: crate::connectivity::core::register_message::RegisterHeaderFields {
                    authorization,
                    contact,
                    accept_contacts: variant
                        .header_profile
                        .include_accept_contact
                        .then(|| {
                            vec![
                                "*;+g.3gpp.smsip".to_string(),
                                format!("*;+g.3gpp.icsi-ref=\"{IMS_MMTEL_ICSI_REF}\""),
                            ]
                        })
                        .unwrap_or_default(),
                    route: variant.include_route_header.then(|| {
                        format!(
                            "<sip:{}:{};lr>",
                            sip_host(self.route_addr),
                            profile.ims.local_port
                        )
                    }),
                    expires: params.expires,
                    supported: Some(params.supported_header),
                    require_sec_agree: params.require_sec_agree,
                    // RFC 3329 §2.3: Require and Proxy-Require travel together.
                    proxy_require_sec_agree: params.require_sec_agree
                        && (profile.ims.register.proxy_require_sec_agree_headers
                            || variant.server_required_sec_agree),
                    allow: Some(params.allow_header),
                    preferred_service: None,
                    preferred_identity: variant
                        .header_profile
                        .include_p_preferred_identity
                        .then(|| format!("<{}>", self.identity.public_uri)),
                    visited_network: params.visited_network,
                    access_network_info: params.pani,
                    cellular_network_info,
                    security_client: (profile.ims.register.sec_agree_mode != "disabled"
                        && (variant.include_security_client || security_verify.is_some()))
                    .then(|| {
                        self.security_client_header(variant.security_client_format)
                            .to_string()
                    }),
                    security_verify: (profile.ims.register.sec_agree_mode != "disabled")
                        .then(|| security_verify.map(str::to_string))
                        .flatten(),
                    user_agent: params.user_agent,
                },
            },
        );
        String::from_utf8(frame).expect("REGISTER builder emits UTF-8 headers")
    }

    fn request_uri(
        &self,
        profile: &'static CarrierProfile,
        variant: LiveRegisterHeaderVariant,
    ) -> String {
        match variant.request_uri {
            LiveRegisterRequestUri::HomeDomain => {
                format!("sip:{}", self.target.domain)
            }
            LiveRegisterRequestUri::HomeRegistrar => {
                let route_domain = self
                    .target
                    .registrar
                    .as_deref()
                    .unwrap_or(&self.target.domain);
                if route_domain.starts_with("sip:") || route_domain.starts_with("sips:") {
                    route_domain.to_string()
                } else {
                    format!("sip:{route_domain}")
                }
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
            LiveInitialAuthorizationFormat::AkaEmpty => Some(
                crate::connectivity::core::digest_aka::build_initial_authorization_header(
                    &self.identity.private_user,
                    &self.target.realm,
                    &self.request_uri(profile, variant),
                ),
            ),
            LiveInitialAuthorizationFormat::AkaEmptyUriFirst => Some(
                crate::connectivity::core::digest_aka::build_initial_authorization_header_uri_first(
                    &self.identity.private_user,
                    &self.target.realm,
                    &self.request_uri(profile, variant),
                ),
            ),
            LiveInitialAuthorizationFormat::AkaEmptyUriFirstNoAlgorithm => Some(format!(
                "Authorization: Digest uri=\"{}\",username=\"{}\",response=\"\",realm=\"{}\",nonce=\"\"",
                quote_sip_param(&self.request_uri(profile, variant)),
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(&self.target.realm)
            )),
            LiveInitialAuthorizationFormat::AkaZeroResponse => Some(format!(
                "Authorization: Digest username=\"{}\",realm=\"{}\",nonce=\"\",uri=\"{}\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(&self.target.realm),
                quote_sip_param(&self.request_uri(profile, variant))
            )),
            LiveInitialAuthorizationFormat::AkaZeroResponseUriFirst => Some(format!(
                "Authorization: Digest uri=\"{}\",username=\"{}\",algorithm=AKAv1-MD5,response=\"00000000000000000000000000000000\",realm=\"{}\",nonce=\"\"",
                quote_sip_param(&self.request_uri(profile, variant)),
                quote_sip_param(&self.identity.private_user),
                quote_sip_param(&self.target.realm)
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

    fn contact_feature_count(
        &self,
        profile: &'static CarrierProfile,
        header_profile: LiveRegisterHeaderProfile,
    ) -> usize {
        if header_profile.compact_register {
            return 0;
        }
        let contact_access_type = access_type_token(profile.ims.register.access_network_info)
            .unwrap_or_else(|| "IEEE-802.11".to_string());
        complete_contact_parameters(ContactCompletion {
            mode: profile.ims.register.contact_mode,
            explicit: profile.ims.register.contact_param_order,
            access_network_info: &contact_access_type,
            include_mmtel: matches!(
                header_profile.contact_features,
                LiveContactFeatureSet::MmtelSmsSipInstance
            ),
            include_video: self.video_capability_enabled,
            include_sip_instance: true,
            always_add_sip_instance: profile.ims.register.always_add_sip_instance,
            sip_instance: &self.instance_id,
            reg_id: WLAN_REG_ID,
            expires: None,
        })
        .len()
    }

    fn build_contact_header(
        &self,
        profile: &'static CarrierProfile,
        local_host: &str,
        header_profile: LiveRegisterHeaderProfile,
    ) -> String {
        let contact_port = self.protected_header_port.unwrap_or(self.local_addr.port());
        let user_phone = if self.identity.contact_user_phone {
            ";user=phone"
        } else {
            ""
        };
        // The Contact transport must match the actual channel transport. The
        // channel follows profile.ims.transport (UDP by default, TCP when a
        // carrier configures it), so echoing that value keeps the REGISTER
        // self-consistent.
        let mut header = format!(
            "Contact: <sip:{}@{}:{}{};transport={}>",
            self.identity.contact_user,
            local_host,
            contact_port,
            user_phone,
            self.transport.as_param()
        );
        // The feature-set fallback below used to be #[cfg(test)] only, so a
        // release build emitted a Contact with no feature tags whenever the
        // profile carried an empty contact_param_order -- which is every
        // hardcoded profile and every catalog bundle without
        // `sip.common.contact_parameters`. The S-CSCF then saw no
        // +g.3gpp.icsi-ref and never treated the registration as MMTEL
        // voice capable. contact_features already mirrors
        // register.include_mmtel_features, so honour it in all builds.
        if !header_profile.compact_register {
            let contact_access_type = access_type_token(profile.ims.register.access_network_info)
                .unwrap_or_else(|| "IEEE-802.11".to_string());
            let parameters = complete_contact_parameters(ContactCompletion {
                mode: profile.ims.register.contact_mode,
                explicit: profile.ims.register.contact_param_order,
                access_network_info: &contact_access_type,
                include_mmtel: matches!(
                    header_profile.contact_features,
                    LiveContactFeatureSet::MmtelSmsSipInstance
                ),
                include_video: self.video_capability_enabled,
                include_sip_instance: true,
                always_add_sip_instance: profile.ims.register.always_add_sip_instance,
                sip_instance: &self.instance_id,
                reg_id: WLAN_REG_ID,
                expires: None,
            });
            for parameter in parameters {
                header.push(';');
                header.push_str(&parameter);
            }
        }
        header.push_str("\r\n");
        header
    }
}

struct VowifiUnregisterFactory {
    line_id: String,
    profile: &'static CarrierProfile,
    context: LiveRegisterRequestContext,
    variant: LiveRegisterHeaderVariant,
    next_cseq: u32,
    security_verify: Option<String>,
}

impl super::operator::RegisteredUnregister for VowifiUnregisterFactory {
    fn initial_request(&self) -> Result<Vec<u8>, ImsError> {
        Ok(self
            .context
            .build_unregister_request(
                self.profile,
                self.variant,
                self.next_cseq,
                None,
                self.security_verify.as_deref(),
            )
            .into_bytes())
    }

    fn authenticated_request<'a>(
        &'a self,
        challenge_response: &'a [u8],
        challenge_cseq: u32,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<u8>, ImsError>> {
        Box::pin(async move {
            let response = std::str::from_utf8(challenge_response)
                .map_err(|_| ImsError::new("vowifi_unregister_response_not_utf8"))?;
            let challenge = parse_live_digest_challenge(response, &self.context.target.realm)
                .map_err(|_| ImsError::new("vowifi_unregister_challenge_invalid"))?;
            reject_plain_digest_when_disabled(self.profile, &challenge)
                .map_err(|_| ImsError::new("vowifi_unregister_digest_rejected"))?;
            let mut material = build_live_register_auth_material(
                &self.line_id,
                self.profile,
                &self.context,
                &challenge,
                self.variant,
            )
            .await
            .map_err(|_| ImsError::new("vowifi_unregister_aka_failed"))?;
            let authorization = match material.auts.take() {
                Some(auts) => build_digest_resync_authorization_header(
                    &self.context,
                    &challenge,
                    &self.context.request_uri(self.profile, self.variant),
                    &auts,
                )
                .map_err(|_| ImsError::new("vowifi_unregister_resync_failed"))?,
                None => material.authorization,
            };
            let cseq = self
                .next_cseq
                .saturating_add(challenge_cseq.saturating_sub(1));
            Ok(self
                .context
                .build_unregister_request(
                    self.profile,
                    self.variant,
                    cseq,
                    Some(&authorization),
                    self.security_verify.as_deref(),
                )
                .into_bytes())
        })
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
            // RFC 3329 `spi-value = 1*8HEXDIG`. Decimal digits are a subset of
            // HEXDIG, so an 8-digit decimal value is unambiguous to both
            // decimal- and hex-parsing peers and cannot be rejected on length.
            spi_c: bounded_spi()?,
            spi_s: bounded_spi()?,
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
    let imsi = effective_imsi_for_line(line_id, &sim.imsi);
    let imsi = imsi.trim();
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
    let target = live_ims_target(line_id, profile);

    Ok(match format {
        LiveRegisterIdentityFormat::ImsiHomeDomain => LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: format!("{imsi}@{}", target.realm),
                public_uri: format!("sip:{imsi}@{}", target.domain),
                contact_user: imsi.to_string(),
                home_domain: target.domain.clone(),
                contact_user_phone: false,
            },
            shape: "imsi_home_domain",
        },
        LiveRegisterIdentityFormat::PrefixedImsiHomeDomain => {
            let prefixed = format!("0{imsi}");
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: format!("{prefixed}@{}", target.realm),
                    public_uri: format!("sip:{prefixed}@{}", target.domain),
                    contact_user: prefixed,
                    home_domain: target.domain.clone(),
                    contact_user_phone: false,
                },
                shape: "prefixed_imsi_home_domain",
            }
        }
        LiveRegisterIdentityFormat::ImsiPhoneUri => LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: format!("{imsi}@{}", target.realm),
                public_uri: format!("sip:{imsi}@{};user=phone", target.domain),
                contact_user: imsi.to_string(),
                home_domain: target.domain.clone(),
                contact_user_phone: true,
            },
            shape: "imsi_phone_uri",
        },
        LiveRegisterIdentityFormat::MsisdnPhoneUri => {
            let phone_number = read_live_msisdn_candidate(line_id, &conn).await?;
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: format!("{imsi}@{}", target.realm),
                    public_uri: format!("sip:{}@{};user=phone", phone_number, target.domain),
                    contact_user: phone_number,
                    home_domain: target.domain.clone(),
                    contact_user_phone: true,
                },
                shape: "msisdn_phone_uri",
            }
        }
    })
}

async fn read_live_msisdn_candidate(
    line_id: &str,
    conn: &zbus::Connection,
) -> Result<String, LiveStageError> {
    let info = line_sim_info(line_id, conn)
        .await
        .ok_or_else(|| live_stage_error("ims_msisdn_unavailable"))?;
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
    /// Integrity-only ESP secrets (ealg=null). Some P-CSCFs (e.g. the
    /// common Kamailio/OpenSIPS integrity-only deployments) ignore the
    /// negotiated AES-CBC ealg and only verify the ICV over the cleartext
    /// payload; probing this variant matches the working beta8 stack.
    ims_esp_null_secrets: Option<ChildSaSecretPair>,
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
    let (response, ims_esp_secrets, ims_esp_alt_secrets, ims_esp_null_secrets) = match challenge
        .nonce_kind
    {
        LiveDigestNonceKind::AkaChallenge => {
            let aka_result =
                authenticate_live_sim_for_line(line_id, &challenge.rand, &challenge.autn)
                    .await
                    .map_err(live_stage_error)?;
            if let Some(auts) = aka_result.auts {
                return Ok(LiveRegisterAuthMaterial {
                    authorization: String::new(),
                    ims_esp_secrets: placeholder_ims_esp_secrets(),
                    ims_esp_alt_secrets: Vec::new(),
                    ims_esp_null_secrets: None,
                    auts: Some(auts),
                });
            }
            if aka_result.res.is_empty() {
                return Err(live_stage_error("ims_aka_empty_response"));
            }
            let response = compute_aka_digest_response(
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
            let null_secrets = derive_ims_esp_secrets_null_encryption(&selected_offer, &aka_result)
                .map(Some)
                .unwrap_or(None);
            (response, secrets, alt_secrets, null_secrets)
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
            (response, placeholder_ims_esp_secrets(), Vec::new(), None)
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
        ims_esp_null_secrets,
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
        && !algorithm.eq_ignore_ascii_case("AKAv2-SHA-256")
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
    let required_param = |name: &str| {
        param(name)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| live_stage_error("ims_security_server_parameter_missing"))
    };
    let alg = required_param("alg")?.to_ascii_lowercase();
    let ealg = required_param("ealg")?.to_ascii_lowercase();
    let protocol = required_param("prot")?.to_ascii_lowercase();
    let mode = required_param("mod")?.to_ascii_lowercase();
    let spi_c = parse_u32_param(param("spi-c"))
        .ok_or_else(|| live_stage_error("ims_security_server_spi_missing"))?;
    let spi_s = parse_u32_param(param("spi-s"))
        .ok_or_else(|| live_stage_error("ims_security_server_spi_missing"))?;
    let port_c = parse_u16_param(param("port-c"))
        .ok_or_else(|| live_stage_error("ims_security_server_port_missing"))?;
    let port_s = parse_u16_param(param("port-s"))
        .ok_or_else(|| live_stage_error("ims_security_server_port_missing"))?;
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
            let alg = parts.next().unwrap_or_default();
            let ealg = parts.next().unwrap_or_default();
            let protocol = parts.next().unwrap_or_default();
            let mode = parts.next().unwrap_or_default();
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

    fn shared(&self) -> crate::connectivity::core::digest_aka::DigestChallenge {
        crate::connectivity::core::digest_aka::DigestChallenge {
            realm: self.realm.clone(),
            nonce: self.nonce.clone(),
            algorithm: self.algorithm.clone(),
            qop: self.qop.map(str::to_string),
            opaque: self.opaque.clone(),
            proxy: self.header_kind == "proxy_authenticate",
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
    crate::connectivity::core::digest_aka::split_digest_challenge_values(value)
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
    crate::connectivity::core::digest_aka::decode_digest_nonce(value)
        .map_err(map_shared_digest_error)
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
fn compute_aka_digest_response(
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
    let material = crate::connectivity::core::digest_aka::AkaMaterial {
        res: &aka.res,
        ck: &aka.ck,
        ik: &aka.ik,
    };
    crate::connectivity::core::digest_aka::compute_aka_response(
        username, realm, &material, algorithm, method, digest_uri, nonce, qop, cnonce, "00000001",
    )
    .map_err(map_shared_digest_error)
}

/// Build one policy-independent SIP Digest-AKA proof for a non-REGISTER
/// transaction on this line. The challenge nonce is consumed immediately and
/// never cached or shared with another SIP method/access.
pub(crate) async fn build_line_sip_aka_authorization(
    line_id: &str,
    username: &str,
    method: &str,
    digest_uri: &str,
    challenge_frame: &[u8],
) -> Result<String, LiveStageError> {
    let www_values =
        crate::connectivity::core::sip_frame::header_values(challenge_frame, "WWW-Authenticate");
    let proxy_values =
        crate::connectivity::core::sip_frame::header_values(challenge_frame, "Proxy-Authenticate");
    // Non-REGISTER transactions on this path always authenticate through the
    // USIM. Plain MD5 has no credential source here and must not be selected.
    let challenge = crate::connectivity::core::digest_aka::select_digest_challenge(
        &www_values,
        &proxy_values,
        false,
    )
    .map_err(map_shared_digest_error)?;
    build_line_parsed_digest_aka_authorization(line_id, username, method, digest_uri, challenge)
        .await
}

async fn build_line_digest_aka_authorization(
    line_id: &str,
    username: &str,
    method: &str,
    digest_uri: &str,
    challenge_value: &str,
    proxy: bool,
) -> Result<String, LiveStageError> {
    let challenge =
        crate::connectivity::core::digest_aka::parse_digest_challenge(challenge_value, proxy)
            .map_err(map_shared_digest_error)?;
    build_line_parsed_digest_aka_authorization(line_id, username, method, digest_uri, challenge)
        .await
}

async fn build_line_parsed_digest_aka_authorization(
    line_id: &str,
    username: &str,
    method: &str,
    digest_uri: &str,
    challenge: crate::connectivity::core::digest_aka::DigestChallenge,
) -> Result<String, LiveStageError> {
    let aka_challenge = crate::connectivity::core::digest_aka::decode_aka_nonce(&challenge.nonce)
        .map_err(map_shared_digest_error)?;
    let aka = authenticate_live_sim_for_line(line_id, &aka_challenge.rand, &aka_challenge.autn)
        .await
        .map_err(live_stage_error)?;
    let cnonce = live_digest_cnonce()?;
    if let Some(auts) = aka.auts.as_deref() {
        return Ok(
            crate::connectivity::core::digest_aka::build_resync_authorization_header_with_digest(
                &challenge,
                username,
                digest_uri,
                auts,
                challenge.qop.as_ref().map(|_| cnonce.as_str()),
                challenge.qop.as_ref().map(|_| "00000001"),
            ),
        );
    }
    let response = compute_aka_digest_response(
        username,
        &challenge.realm,
        &aka,
        &challenge.algorithm,
        method,
        digest_uri,
        &challenge.nonce,
        challenge.qop.as_deref(),
        &cnonce,
    )?;
    Ok(
        crate::connectivity::core::digest_aka::build_authorization_header(
            &challenge, username, digest_uri, &response, &cnonce, "00000001",
        ),
    )
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

fn map_shared_digest_error(error: ImsError) -> LiveStageError {
    let reason = match error.code() {
        "aka_res_empty" => "ims_aka_empty_response",
        "aka_material_invalid" => "ims_aka_material_invalid",
        "digest_algorithm_unsupported" => "ims_digest_algorithm_unsupported",
        "digest_qop_unsupported" => "ims_digest_qop_unsupported",
        "digest_nonce_decode_failed" | "hex_invalid" => "ims_digest_nonce_decode_failed",
        other => other,
    };
    live_stage_error(reason)
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

/// Derive integrity-only ESP secrets (ealg=null, hmac-sha-1-96) while
/// keeping the SPIs/ports negotiated in the Security-Server offer. This
/// probes P-CSCFs that only perform ESP integrity protection regardless of
/// the ealg advertised in the Security-Server header.
fn derive_ims_esp_secrets_null_encryption(
    offer: &LiveSecurityServerOffer,
    aka: &super::qmi_uim::UsimAkaApduResult,
) -> Result<ChildSaSecretPair, LiveStageError> {
    let mut null_offer = offer.clone();
    null_offer.ealg = "null".to_string();
    derive_ims_esp_secrets_with_integrity_key(&null_offer, aka, true)
}

fn derive_ims_esp_secrets_with_integrity_key(
    offer: &LiveSecurityServerOffer,
    aka: &super::qmi_uim::UsimAkaApduResult,
    expand_ik_to_hmac_sha1_key: bool,
) -> Result<ChildSaSecretPair, LiveStageError> {
    if !offer.alg.eq_ignore_ascii_case("hmac-sha-1-96")
        || !(offer.ealg.eq_ignore_ascii_case("aes-cbc") || offer.ealg.eq_ignore_ascii_case("null"))
        || !offer.protocol.eq_ignore_ascii_case("esp")
        || !offer.mode.eq_ignore_ascii_case("trans")
    {
        return Err(live_stage_error("ims_security_server_offer_unsupported"));
    }
    if aka.ck.len() < 16 || aka.ik.len() < 16 {
        return Err(live_stage_error("ims_aka_material_invalid"));
    }
    let integrity_key = if expand_ik_to_hmac_sha1_key {
        ims_hmac_sha1_96_key(&aka.ik[..16])
    } else {
        aka.ik[..16].to_vec()
    };
    let (encryption, encryption_key_bytes, encryption_key) =
        if offer.ealg.eq_ignore_ascii_case("null") {
            ("null", 0usize, Vec::new())
        } else {
            ("aes_cbc", 16usize, aka.ck[..16].to_vec())
        };
    let plan = ChildSaKeySchedulePlan {
        encryption,
        integrity: "hmac_sha1_96",
        encryption_key_bytes,
        integrity_key_bytes: integrity_key.len(),
        direction_secret_bytes: encryption_key_bytes + integrity_key.len(),
        total_secret_bytes: (encryption_key_bytes + integrity_key.len()) * 2,
        exported_secret_values: false,
        sensitive_values_policy: "ims_ipsec3gpp_secret_bytes_redacted_and_zeroed_on_drop",
    };
    if std::env::var("SIMADMIN_DEBUG_ESP_KEYS").is_ok() {
        let hex_bytes = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        tracing::info!(
            ck_hex = hex_bytes(&aka.ck[..16]),
            ik_hex = hex_bytes(&aka.ik[..16]),
            integrity_key_hex = hex_bytes(&integrity_key),
            encryption_key_hex = hex_bytes(&encryption_key),
            expand_ik = expand_ik_to_hmac_sha1_key,
            "IMS ESP key material (SIMADMIN_DEBUG_ESP_KEYS debug dump)"
        );
    }
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
    Ok(
        crate::connectivity::core::digest_aka::build_authorization_header(
            &challenge.shared(),
            &context.identity.private_user,
            digest_uri,
            response,
            cnonce,
            "00000001",
        ),
    )
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
    let cnonce = challenge.qop.map(|_| live_digest_cnonce()).transpose()?;
    Ok(
        crate::connectivity::core::digest_aka::build_resync_authorization_header_with_digest(
            &challenge.shared(),
            &context.identity.private_user,
            digest_uri,
            auts,
            cnonce.as_deref(),
            Some("00000001"),
        ),
    )
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

/// `+sip.instance` for this line's VoWiFi registration.
///
/// Derived from the IMPI rather than randomised per registration, so this leg
/// and the VoLTE leg present the *same* instance id for the same subscription.
/// Two different ids for one IMPU leave the S-CSCF holding two independent RFC
/// 5626 bindings, and terminating calls then land on whichever one the TAS
/// picks. See [`stable_sip_instance`] for the full reasoning.
///
/// [`stable_sip_instance`]: crate::connectivity::core::device_identity::stable_sip_instance
fn format_sip_instance_id(
    profile: &'static CarrierProfile,
    identity: &crate::connectivity::core::context::ImsIdentity,
    device_imei: Option<&str>,
) -> String {
    crate::connectivity::core::device_identity::stable_sip_instance(
        &identity.private_user,
        device_imei,
        profile.identity.device_identity_enabled && profile.ims.register.always_add_sip_instance,
    )
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
        .unwrap_or_default();
    let mut parts = mechanism.split('/');
    let alg = parts.next().unwrap_or_default();
    let ealg = parts.next().unwrap_or_default();
    let protocol = parts.next().unwrap_or_default();
    let mode = parts.next().unwrap_or_default();
    // Field-tested against the Maxis P-CSCF: the quoted form `mod="trans"`
    // is rejected with "400 Bad header field: security-client", while real
    // Android UEs send the unquoted token `mod=trans` and are accepted.
    // SPI values stay 8-digit decimal (see LiveSecurityClientState), which
    // parses identically as decimal or hex and satisfies 1*8HEXDIG.
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
fn sec_agree_headers_required(
    profile: &'static CarrierProfile,
    force_from_variant: bool,
    suppress_from_variant: bool,
) -> bool {
    if suppress_from_variant {
        return false;
    }
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
    sanitize_header_value(profile.ims.register.access_network_info)
        .unwrap_or_else(|| "IEEE-802.11".to_string())
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

/// Random SPI in 1..=99_999_999 (max 8 decimal digits, i.e. always a valid
/// RFC 3329 `1*8HEXDIG` value whose decimal and hex interpretations agree).
fn bounded_spi() -> Result<u32, LiveStageError> {
    let value = random_u32_nonzero()? % 100_000_000;
    Ok(if value == 0 { 1 } else { value })
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
    let imsi = effective_imsi_for_line(line_id, &sim.imsi);
    build_permanent_nai(profile, &imsi).map_err(map_identity_error)
}

fn map_identity_error(error: IkeIdentityError) -> LiveStageError {
    live_stage_error(match error {
        IkeIdentityError::EmptyImsi | IkeIdentityError::InvalidImsi => "ike_identity_unavailable",
        IkeIdentityError::ImsiPlmnMismatch => "ike_identity_profile_mismatch",
        IkeIdentityError::PrivateIdentityTemplateRequired => {
            "ike_identity_private_template_required"
        }
        IkeIdentityError::InvalidIdentityTemplate => "ike_identity_template_invalid",
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
        registration_loss: None,
        server_required_sec_agree: false,
        register_auth_rounds: 0,
    }
}

fn live_registration_error(
    reason: impl Into<String>,
    registration_loss: RegistrationLossReason,
) -> LiveStageError {
    LiveStageError {
        reason: reason.into(),
        registration_loss: Some(registration_loss),
        server_required_sec_agree: false,
        register_auth_rounds: 0,
    }
}

impl LiveStageError {
    fn with_registration_loss(mut self, registration_loss: RegistrationLossReason) -> Self {
        self.registration_loss = Some(registration_loss);
        self
    }
}

#[derive(Debug, Clone)]
pub struct LiveNetworkStageAdapter<E, D> {
    line_id: String,
    epdg: E,
    datagram: D,
}

impl<E, D> LiveNetworkStageAdapter<E, D> {
    pub fn new(epdg: E, datagram: D) -> Self {
        Self::for_line(String::new(), epdg, datagram)
    }

    pub fn for_line(line_id: impl Into<String>, epdg: E, datagram: D) -> Self {
        Self {
            line_id: line_id.into(),
            epdg,
            datagram,
        }
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
                    let identity = line_sim_identity(&self.line_id, &conn)
                        .await
                        .ok_or_else(|| live_stage_error("sim_identity_not_ready"))?;
                    if identity.imsi.is_empty() {
                        return Err(live_stage_error("sim_imsi_empty"));
                    }
                    verify_live_sim_auth_access_for_line(&self.line_id).await?;
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

    fn epdg_selection_entry(
        plmn_pattern: &str,
        priority: u16,
        fqdn_format: EpdgFqdnFormat,
    ) -> super::super::qmi_uim::UsimEpdgSelectionEntry {
        super::super::qmi_uim::UsimEpdgSelectionEntry {
            plmn_pattern: plmn_pattern.to_string(),
            priority,
            fqdn_format,
        }
    }

    fn epdg_candidate_hosts(candidates: &[EpdgEndpointCandidate]) -> Vec<String> {
        candidates.iter().map(EpdgEndpointCandidate::host).collect()
    }

    #[test]
    fn line_epdg_override_is_strict_canonical_and_unique() {
        let line_id = "test-vowifi-strict-epdg-override";
        let config = LineVowifiConfig::default();
        let sim_override = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                epdg_host: Some("Pinned.EPDG.Example.".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        configure_live_network_overrides(line_id, &config, Some(&sim_override))
            .expect("publish strict ePDG override");
        let uicc = UsimEpdgConfig {
            home_identifiers: vec![UsimEpdgAddress::Fqdn("epdg.from-uicc.example".to_string())],
            selection: vec![epdg_selection_entry(
                "23433",
                1,
                EpdgFqdnFormat::OperatorIdentifier,
            )],
        };

        let candidates = build_live_epdg_candidates(
            line_id,
            &profiles::GB_EE_23433,
            &uicc,
            Some(&EpdgLocationSnapshot {
                serving_plmn: "23433".to_string(),
                technology: "lte".to_string(),
                tac: 0x1234,
            }),
        );
        assert_eq!(epdg_candidate_hosts(&candidates), ["pinned.epdg.example"]);
        assert_eq!(candidates[0].source, EpdgCandidateSource::LineOverride);

        forget_live_network_overrides(line_id);
    }

    #[test]
    fn provisioned_profile_does_not_gain_an_unrequested_public_dns_guess() {
        let candidates = build_live_epdg_candidates(
            "test-vowifi-provisioned-profile-only",
            &profiles::NZ_SPARK_53005,
            &UsimEpdgConfig::default(),
            None,
        );
        assert_eq!(
            epdg_candidate_hosts(&candidates),
            [profiles::NZ_SPARK_53005.epdg.host]
        );
        assert_eq!(candidates[0].source, EpdgCandidateSource::CarrierProfile);
        assert_ne!(
            profiles::NZ_SPARK_53005.epdg.host,
            profiles::standard_operator_epdg_fqdn("530", "05").unwrap()
        );
    }

    #[test]
    fn derived_epdg_candidates_follow_uicc_selection_home_id_then_hplmn_order() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let uicc = UsimEpdgConfig {
            home_identifiers: vec![UsimEpdgAddress::Fqdn("epdg.from-uicc.example".to_string())],
            selection: vec![epdg_selection_entry(
                "50212",
                1,
                EpdgFqdnFormat::LocationBased,
            )],
        };
        let location = EpdgLocationSnapshot {
            serving_plmn: "50212".to_string(),
            technology: "lte".to_string(),
            tac: 0x0b21,
        };

        let candidates = build_live_epdg_candidates(
            "test-vowifi-derived-order",
            profile,
            &uicc,
            Some(&location),
        );
        assert_eq!(
            epdg_candidate_hosts(&candidates),
            [
                "tac-lb21.tac-hb0b.tac.epdg.epc.mnc012.mcc502.pub.3gppnetwork.org",
                "epdg.from-uicc.example",
                "epdg.epc.mnc012.mcc502.pub.3gppnetwork.org",
            ]
        );
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.source)
                .collect::<Vec<_>>(),
            [
                EpdgCandidateSource::UiccSelection,
                EpdgCandidateSource::UiccHomeIdentifier,
                EpdgCandidateSource::HomePlmnDerived,
            ]
        );
    }

    #[test]
    fn roaming_epdg_selection_prefers_exact_vplmn_over_any_plmn() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let uicc = UsimEpdgConfig {
            home_identifiers: vec![UsimEpdgAddress::Fqdn("epdg.home.example".to_string())],
            selection: vec![
                epdg_selection_entry("DDDDDD", 0, EpdgFqdnFormat::LocationBased),
                epdg_selection_entry("310260", 10, EpdgFqdnFormat::OperatorIdentifier),
                epdg_selection_entry("50212", 1, EpdgFqdnFormat::OperatorIdentifier),
            ],
        };
        let location = EpdgLocationSnapshot {
            serving_plmn: "310260".to_string(),
            technology: "lte".to_string(),
            tac: 0x1234,
        };

        let candidates = build_live_epdg_candidates(
            "test-vowifi-roaming-exact-vplmn",
            profile,
            &uicc,
            Some(&location),
        );
        assert_eq!(
            epdg_candidate_hosts(&candidates),
            [
                "epdg.epc.mnc260.mcc310.pub.3gppnetwork.org",
                "epdg.home.example",
                profile.epdg.host,
            ]
        );
        assert_eq!(candidates[0].source, EpdgCandidateSource::UiccSelection);
    }

    #[test]
    fn roaming_epdg_selection_uses_any_plmn_format_for_the_serving_plmn() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let uicc = UsimEpdgConfig {
            home_identifiers: Vec::new(),
            selection: vec![epdg_selection_entry(
                "DDDDDD",
                1,
                EpdgFqdnFormat::LocationBased,
            )],
        };
        let location = EpdgLocationSnapshot {
            serving_plmn: "310260".to_string(),
            technology: "nr".to_string(),
            tac: 0x0b1a21,
        };

        let candidates = build_live_epdg_candidates(
            "test-vowifi-roaming-any-plmn",
            profile,
            &uicc,
            Some(&location),
        );
        assert_eq!(
            epdg_candidate_hosts(&candidates),
            [
                "tac-lb21.tac-mb1a.tac-hb0b.5gstac.epdg.epc.mnc260.mcc310.pub.3gppnetwork.org",
                profile.epdg.host,
            ]
        );
    }

    #[test]
    fn roaming_private_plmn_never_receives_a_public_standard_epdg_name() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let uicc = UsimEpdgConfig {
            home_identifiers: vec![UsimEpdgAddress::Ip("192.0.2.55".parse().unwrap())],
            selection: vec![epdg_selection_entry(
                "DDDDDD",
                1,
                EpdgFqdnFormat::OperatorIdentifier,
            )],
        };
        let location = EpdgLocationSnapshot {
            serving_plmn: "99999".to_string(),
            technology: "lte".to_string(),
            tac: 0x0021,
        };

        let candidates = build_live_epdg_candidates(
            "test-vowifi-roaming-private-plmn",
            profile,
            &uicc,
            Some(&location),
        );
        let hosts = epdg_candidate_hosts(&candidates);
        assert_eq!(hosts, ["192.0.2.55", profile.epdg.host]);
        assert!(hosts.iter().all(|host| !host.contains("mcc999")));
    }

    #[test]
    fn home_location_epdg_requires_fresh_explicit_hplmn_selection_and_ignores_any_plmn() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let uicc = UsimEpdgConfig {
            home_identifiers: Vec::new(),
            selection: vec![
                epdg_selection_entry("50212", 1, EpdgFqdnFormat::LocationBased),
                epdg_selection_entry("DDDDDD", 0, EpdgFqdnFormat::OperatorIdentifier),
            ],
        };

        let no_snapshot =
            build_live_epdg_candidates("test-vowifi-location-none", profile, &uicc, None);
        assert_eq!(epdg_candidate_hosts(&no_snapshot), [profile.epdg.host]);
        assert_eq!(no_snapshot[0].source, EpdgCandidateSource::HomePlmnDerived);

        let runtime = ImsAccessNetworkRuntime::default();
        runtime.publish(
            crate::connectivity::core::access_network::ServingAccessSnapshot::new(
                "502",
                "12",
                "lte",
                0x12345,
                0x0b21,
                Some("B3".to_string()),
                crate::connectivity::core::access_network::AccessNetworkSource::TestFixture,
            )
            .expect("fresh modem snapshot"),
        );
        std::thread::sleep(Duration::from_millis(1));
        let stale = runtime.epdg_location_with_max_age(Duration::ZERO);
        assert!(stale.is_none());
        let stale_candidates = build_live_epdg_candidates(
            "test-vowifi-location-stale",
            profile,
            &uicc,
            stale.as_ref(),
        );
        assert_eq!(epdg_candidate_hosts(&stale_candidates), [profile.epdg.host]);
    }

    #[test]
    fn epdg_candidates_reject_emergency_names_deduplicate_and_stay_bounded() {
        let profile = profiles::derive_standard_3gpp_profile(
            "502",
            "12",
            profiles::Standard3gppAccess::WifiEpdg,
        )
        .expect("public standard profile");
        let mut home_identifiers = vec![
            UsimEpdgAddress::Fqdn("sos.epdg.invalid.example".to_string()),
            UsimEpdgAddress::Fqdn("EPDG0.EXAMPLE.ORG.".to_string()),
            UsimEpdgAddress::Fqdn("epdg0.example.org".to_string()),
        ];
        home_identifiers.extend(
            (1..=10).map(|index| UsimEpdgAddress::Fqdn(format!("epdg{index}.example.org"))),
        );
        let candidates = build_live_epdg_candidates(
            "test-vowifi-candidate-bounds",
            profile,
            &UsimEpdgConfig {
                home_identifiers,
                selection: Vec::new(),
            },
            None,
        );
        let hosts = epdg_candidate_hosts(&candidates);
        assert_eq!(hosts.len(), LIVE_EPDG_MAX_HOST_CANDIDATES);
        assert!(hosts.iter().all(|host| !host.starts_with("sos.")));
        let mut unique = hosts.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), hosts.len());
    }

    #[test]
    fn unchanged_reader_refresh_preserves_only_its_lines_uicc_cache() {
        let line_a = "test-vowifi-uicc-cache-line-a";
        let line_b = "test-vowifi-uicc-cache-line-b";
        register_line_sim_device(line_a, "/dev/qmi-a", 1, "/modem/a");
        register_line_sim_device(line_b, "/dev/qmi-b", 2, "/modem/b");
        let device_a = sim_device_for_line(line_a);
        let device_b = sim_device_for_line(line_b);
        {
            let mut cache = live_uicc_epdg_config_cache()
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.insert(
                line_a.to_string(),
                CachedLiveUiccEpdgConfig {
                    device: device_a.clone(),
                    loaded_at: Instant::now(),
                    config: UsimEpdgConfig {
                        home_identifiers: vec![UsimEpdgAddress::Fqdn(
                            "epdg.line-a.example".to_string(),
                        )],
                        selection: Vec::new(),
                    },
                },
            );
            cache.insert(
                line_b.to_string(),
                CachedLiveUiccEpdgConfig {
                    device: device_b,
                    loaded_at: Instant::now(),
                    config: UsimEpdgConfig::default(),
                },
            );
        }

        register_line_sim_device(line_a, "/dev/qmi-a", 1, "/modem/a");
        {
            let cache = live_uicc_epdg_config_cache()
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(cache.contains_key(line_a));
            assert!(cache.contains_key(line_b));
        }

        register_line_sim_device(line_a, "/dev/qmi-a", 3, "/modem/a");
        {
            let cache = live_uicc_epdg_config_cache()
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(!cache.contains_key(line_a));
            assert!(cache.contains_key(line_b));
        }

        forget_line_sim_device(line_a);
        forget_line_sim_device(line_b);
    }

    #[tokio::test]
    async fn refresh_failure_state_is_line_scoped_and_thresholded() {
        let line_a = "test-vowifi-refresh-failure-line-a";
        let line_b = "test-vowifi-refresh-failure-line-b";
        clear_live_ims_refresh_failure_for_line(line_a).await;
        clear_live_ims_refresh_failure_for_line(line_b).await;

        assert_eq!(
            record_live_ims_refresh_failure(line_a, "first").await,
            LiveImsRefreshFailureDecision::Retry
        );
        assert_eq!(
            record_live_ims_refresh_failure(line_a, "second").await,
            LiveImsRefreshFailureDecision::Retry
        );
        assert_eq!(live_ims_refresh_failure_count_for_line(line_a).await, 2);
        assert_eq!(live_ims_refresh_failure_count_for_line(line_b).await, 0);

        assert_eq!(
            record_live_ims_refresh_failure(line_a, "third").await,
            LiveImsRefreshFailureDecision::RebuildAccess
        );
        assert_eq!(
            live_ims_refresh_failure_count_for_line(line_a).await,
            LIVE_IMS_REFRESH_REBUILD_FAILURES
        );
        assert_eq!(
            record_live_ims_refresh_failure(line_a, "saturated").await,
            LiveImsRefreshFailureDecision::RebuildAccess
        );
        assert_eq!(
            live_ims_refresh_failure_count_for_line(line_a).await,
            LIVE_IMS_REFRESH_REBUILD_FAILURES
        );

        mark_live_ims_refresh_rebuild_pending(line_a).await;
        assert!(live_ims_refresh_rebuild_pending_for_line(line_a).await);
        assert_eq!(
            record_live_ims_refresh_failure(line_a, "pending").await,
            LiveImsRefreshFailureDecision::RebuildPending
        );

        clear_live_ims_refresh_failure_for_line(line_a).await;
        assert_eq!(live_ims_refresh_failure_count_for_line(line_a).await, 0);
        assert!(!live_ims_refresh_rebuild_pending_for_line(line_a).await);
        clear_live_ims_refresh_failure_for_line(line_b).await;
    }

    #[test]
    fn named_sim_devices_never_inherit_global_fallbacks() {
        let missing = sim_device_for_line("");
        assert!(missing.qmi_device.is_empty());
        assert!(missing.modem_path.is_empty());

        let unknown = sim_device_for_line("test-unknown-vowifi-line");
        assert!(unknown.qmi_device.is_empty());
        assert!(unknown.modem_path.is_empty());

        let line_id = "test-mbim-only-vowifi-line";
        register_line_sim_device(line_id, "", 2, "/org/freedesktop/ModemManager1/Modem/8");
        let mapped = sim_device_for_line(line_id);
        assert!(mapped.qmi_device.is_empty());
        assert_eq!(mapped.uim_slot, 2);
        assert_eq!(mapped.modem_path, "/org/freedesktop/ModemManager1/Modem/8");
        forget_line_sim_device(line_id);
    }

    #[test]
    fn reader_refresh_preserves_the_atomic_ue_network_context() {
        let line_id = "test-vowifi-refresh-keeps-ue-context";
        let namespace = crate::platform::netns::NetnsName::for_line("sa-ue", line_id);
        let worker = UeWorkerHandle::for_line(line_id, namespace.clone());
        register_line_sim_device(
            line_id,
            "/dev/wwan-test-qmi",
            1,
            "/org/freedesktop/ModemManager1/Modem/11",
        );
        register_line_ue_socket_context(
            line_id,
            Some(LiveUeSocketContext {
                namespace: namespace.as_str().to_string(),
                ue_veth: "save-test".to_string(),
                worker,
            }),
        );

        forget_line_sim_device_mapping(line_id);

        assert!(sim_device_for_line(line_id).qmi_device.is_empty());
        assert_eq!(
            ue_namespace_for_line(line_id).as_deref(),
            Some(namespace.as_str())
        );
        assert_eq!(
            ue_socket_context_for_line(line_id)
                .as_ref()
                .map(|context| context.ue_veth.as_str()),
            Some("save-test")
        );

        forget_line_sim_device(line_id);
        assert!(ue_namespace_for_line(line_id).is_none());
        assert!(ue_socket_context_for_line(line_id).is_none());
    }

    #[test]
    fn pcsc_lines_remain_bound_to_their_exact_reader_without_qmi_fallback() {
        let line_id = "test-pcsc-vowifi-line";
        register_line_pcsc_reader(line_id, "pcsc://ACS ACR38U 00 00");

        let mapped = sim_device_for_line(line_id);
        assert_eq!(mapped.pcsc_reader, "pcsc://ACS ACR38U 00 00");
        assert!(mapped.qmi_device.is_empty());
        assert!(mapped.modem_path.is_empty());
        assert_eq!(mapped.uim_slot, 0);

        forget_line_sim_device(line_id);
    }

    #[test]
    fn live_network_adapter_preserves_its_line_scope() {
        let adapter =
            LiveNetworkStageAdapter::for_line("line-b", MockEpdgAdapter, MockDatagramAdapter);

        assert_eq!(adapter.line_id, "line-b");
    }

    #[test]
    fn live_dns_attempts_end_with_system_resolver_fallback() {
        let line_id = "line-vowifi-dns-system-fallback";
        let config = LineVowifiConfig::default();
        let sim_override = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                dns: Some(vec!["192.0.2.1".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };

        configure_live_network_overrides(line_id, &config, Some(&sim_override))
            .expect("publish DNS override");
        let attempts = live_dns_attempts(line_id, &GB_EE_23433);
        assert_eq!(
            attempts.first(),
            Some(&Some("192.0.2.1:53".parse().unwrap()))
        );
        assert_eq!(attempts.last(), Some(&None));
        forget_live_network_overrides(line_id);
    }

    #[test]
    fn line_network_overrides_apply_custom_dns_and_profile_pin() {
        let config = LineVowifiConfig::default();
        let sim_override = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                dns: Some(vec!["[2001:4860:4860::8888]:5353".to_string()]),
                profile_id: Some("gb_ee_23433".to_string()),
                domain: Some("ims.example".to_string()),
                realm: Some("realm.example".to_string()),
                registrar: Some("sip:registrar.example".to_string()),
                pcscf: Some(vec!["2001:db8::5060".to_string()]),
                epdg_host: Some("epdg.example".to_string()),
                epdg_port: Some(4500),
                apn: Some("ims-override".to_string()),
                ip_stack: Some("ipv4v6".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let overrides = build_live_network_overrides(&config, Some(&sim_override), None)
            .expect("valid network overrides");

        assert_eq!(overrides.profile_id.as_deref(), Some("gb_ee_23433"));
        assert_eq!(
            overrides.dns_servers,
            vec!["[2001:4860:4860::8888]:5353".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(overrides.ims_domain.as_deref(), Some("ims.example"));

        let line_id = "line-effective-vowifi-snapshot";
        configure_live_network_overrides(line_id, &config, Some(&sim_override))
            .expect("publish effective snapshot");
        assert_eq!(
            live_epdg_settings(line_id, &GB_EE_23433),
            (
                "epdg.example".to_string(),
                4500,
                Some("2001:4860:4860::8888".parse::<IpAddr>().unwrap())
            )
        );
        assert_eq!(
            live_ike_access(line_id, &GB_EE_23433),
            IkeAccessConfig {
                ip_stack: "ipv4v6".to_string(),
                apn: Some("ims-override".to_string()),
                epdg_host: "epdg.example".to_string(),
                device_identity: None,
            }
        );
        assert_eq!(
            live_ims_target(line_id, &GB_EE_23433),
            LiveImsTarget {
                domain: "ims.example".to_string(),
                realm: "realm.example".to_string(),
                registrar: Some("sip:registrar.example".to_string()),
                pcscf: vec!["2001:db8::5060".to_string()],
            }
        );
        forget_live_network_overrides(line_id);
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

        // These IDs share their final 24 bits. A six-hex suffix would collide;
        // the 32-bit suffix must keep them on separate TUN devices.
        assert_ne!(
            tun_name_for_line("sa_vwf0", "line-00000000000000000000000012abcdef"),
            tun_name_for_line("sa_vwf0", "line-00000000000000000000000034abcdef")
        );
    }

    #[test]
    fn line_network_overrides_require_a_line_id() {
        assert_eq!(
            configure_live_network_overrides("", &LineVowifiConfig::default(), None).unwrap_err(),
            "line_id_required"
        );
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
            build_live_network_overrides(&config, None, None).unwrap_err(),
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
        let overrides = build_live_network_overrides(&good, None, None).expect("socks5 accepted");
        assert!(matches!(overrides.proxy, Some(LiveProxySetting::Socks5(_))));

        // A malformed endpoint must be rejected at configuration time, not at
        // connect time.
        let bad = LineVowifiConfig {
            proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
            proxy_endpoint: "socks5://missing-port".to_string(),
            ..LineVowifiConfig::default()
        };
        assert!(build_live_network_overrides(&bad, None, None).is_err());
    }

    #[test]
    fn per_line_overrides_do_not_leak_between_lines() {
        // The whole point of keying overrides by line: two SIMs on different
        // operators, each with its own proxy and DNS, must stay independent.
        let japan = LineVowifiConfig {
            enabled: true,
            proxy_mode: VowifiProxyMode::Socks5UdpAssociate,
            proxy_endpoint: "socks5://127.0.0.1:1080".to_string(),
            ..LineVowifiConfig::default()
        };
        let malaysia = LineVowifiConfig {
            enabled: true,
            proxy_mode: VowifiProxyMode::Direct,
            proxy_endpoint: String::new(),
            ..LineVowifiConfig::default()
        };
        let japan_override = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                dns: Some(vec!["1.1.1.1".to_string()]),
                profile_id: Some("jp_carrier".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let malaysia_override = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                dns: Some(vec!["8.8.8.8".to_string()]),
                profile_id: Some("my_carrier".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        configure_live_network_overrides("line-jp", &japan, Some(&japan_override))
            .expect("configure jp");
        configure_live_network_overrides("line-my", &malaysia, Some(&malaysia_override))
            .expect("configure my");

        let jp = line_overrides("line-jp");
        let my = line_overrides("line-my");
        assert_eq!(jp.profile_id.as_deref(), Some("jp_carrier"));
        assert_eq!(my.profile_id.as_deref(), Some("my_carrier"));
        assert_eq!(
            jp.dns_servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["1.1.1.1:53"]
        );
        assert_eq!(
            my.dns_servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["8.8.8.8:53"]
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

    #[test]
    fn presented_imsi_is_scoped_to_one_line_and_requires_enablement() {
        let config = LineVowifiConfig::default();
        let enabled = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                spoof_imsi: true,
                custom_imsi: Some("460001234567890".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let disabled = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                spoof_imsi: false,
                custom_imsi: Some("234331234567890".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        configure_live_network_overrides("line-spoof-enabled", &config, Some(&enabled))
            .expect("configure enabled IMSI override");
        configure_live_network_overrides("line-spoof-disabled", &config, Some(&disabled))
            .expect("configure disabled IMSI override");

        assert_eq!(
            effective_imsi_for_line("line-spoof-enabled", "204041111111111"),
            "460001234567890"
        );
        assert_eq!(
            effective_imsi_for_line("line-spoof-disabled", "204041111111111"),
            "204041111111111"
        );
        assert_eq!(
            effective_imsi_for_line("line-spoof-unknown", "204041111111111"),
            "204041111111111"
        );

        forget_live_network_overrides("line-spoof-enabled");
        forget_live_network_overrides("line-spoof-disabled");
    }

    #[test]
    fn persisted_override_edits_do_not_mutate_the_published_session_snapshot() {
        let line_id = "line-active-snapshot";
        let config = LineVowifiConfig::default();
        let initial = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                epdg_host: Some("epdg.initial.example".to_string()),
                domain: Some("ims.initial.example".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        configure_live_network_overrides(line_id, &config, Some(&initial))
            .expect("publish connection snapshot");

        // Saving a later document updates SimOverrideStore only. Until an
        // explicit reconnect publishes another snapshot, the active session is
        // still anchored to the values captured above.
        let edited = SimOverride {
            ims_vowifi: crate::connectivity::modems::ims::profile_override::ImsAccessOverride {
                epdg_host: Some("epdg.edited.example".to_string()),
                domain: Some("ims.edited.example".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        validate_live_network_overrides(&config, Some(&edited)).expect("validate later edit");

        let active = line_overrides(line_id);
        assert_eq!(active.epdg_host.as_deref(), Some("epdg.initial.example"));
        assert_eq!(active.ims_domain.as_deref(), Some("ims.initial.example"));
        forget_live_network_overrides(line_id);
    }

    fn register_variant(label: &str) -> LiveRegisterHeaderVariant {
        *LIVE_REGISTER_HEADER_VARIANTS
            .iter()
            .find(|variant| variant.label == label)
            .expect("register variant exists")
    }

    fn register_failure_with(response: &str, auth_rounds: u8) -> RegisterFailure {
        RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"),
            response: Some(response.as_bytes().to_vec()),
            auth_rounds,
        }
    }

    /// A bundle that omits `security_agreement` resolves to "auto", so the
    /// first REGISTER offers Security-Client without Require/Proxy-Require and
    /// a strict core answers 421. The next attempt must carry the declaration
    /// rather than repeat the rejected shape.
    #[test]
    fn sec_agree_421_upgrades_the_offering_variant() {
        let failure = register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            0,
        );
        let error = map_shared_register_failure(&failure);
        assert!(error.server_required_sec_agree);

        let variant = register_variant("ims_features_aka_uri_first_full_sec_client");
        assert!(!variant.force_sec_agree_headers);
        let upgraded =
            sec_agree_retry_variant(&GB_EE_23433, variant, &error).expect("variant is upgraded");
        assert!(upgraded.force_sec_agree_headers);
        assert!(upgraded.server_required_sec_agree);
        assert!(!upgraded.suppress_sec_agree_headers);
        assert!(upgraded.include_security_client);
        // The rest of the proven shape must survive the upgrade.
        assert_eq!(
            format!("{:?}", upgraded.request_uri),
            format!("{:?}", variant.request_uri)
        );
        assert_eq!(
            format!("{:?}", upgraded.initial_authorization),
            format!("{:?}", variant.initial_authorization)
        );
        assert_eq!(
            format!("{:?}", upgraded.security_client_format),
            format!("{:?}", variant.security_client_format)
        );
    }

    /// Lock the field-observed Maxis 50212 VoWiFi sequence at the variant and
    /// final SIP-byte layers: 421 adds the complete security agreement, 400
    /// adds URI-first empty AKA without losing it, and the authenticated round
    /// keeps the negotiated declaration.
    #[test]
    fn maxis_50212_vowifi_dynamic_register_upgrade_is_cumulative_end_to_end() {
        let profile =
            crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
                "502",
                "12",
                crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::WifiEpdg,
            )
            .expect("derived Maxis Wi-Fi profile");
        let base = live_register_header_variants(profile)[0];
        assert_eq!(
            base.initial_authorization,
            LiveInitialAuthorizationFormat::None
        );
        assert!(!base.force_sec_agree_headers);
        assert!(!base.server_required_sec_agree);
        assert!(!base.include_security_client);

        let requires_sec_agree = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        let declared = next_dynamic_live_register_variant(profile, base, &requires_sec_agree)
            .expect("421 must preserve the base request and declare sec-agree");
        assert_eq!(
            declared.initial_authorization,
            LiveInitialAuthorizationFormat::None
        );
        assert!(declared.force_sec_agree_headers);
        assert!(declared.server_required_sec_agree);
        assert!(declared.include_security_client);
        assert_eq!(
            declared.security_client_format,
            LiveSecurityClientFormat::FullSpaced
        );
        assert_eq!(declared.include_route_header, base.include_route_header);
        assert_eq!(
            format!("{:?}", declared.request_uri),
            format!("{:?}", base.request_uri)
        );
        assert_eq!(
            format!("{:?}", declared.identity_format),
            format!("{:?}", base.identity_format)
        );
        assert_eq!(
            format!("{:?}", declared.header_profile),
            format!("{:?}", base.header_profile)
        );

        let bad_request = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        let cumulative = next_dynamic_live_register_variant(profile, declared, &bad_request)
            .expect("400 after sec-agree must add the empty AKA hint cumulatively");
        assert_eq!(
            cumulative.initial_authorization,
            LiveInitialAuthorizationFormat::AkaEmptyUriFirst
        );
        assert!(cumulative.force_sec_agree_headers);
        assert!(cumulative.server_required_sec_agree);
        assert!(cumulative.include_security_client);
        assert_eq!(
            cumulative.security_client_format,
            LiveSecurityClientFormat::FullSpaced,
            "empty AKA must be tried before Security-Client formatting fallbacks"
        );

        let context = LiveRegisterRequestContext::new(
            profile,
            LiveImsRegisterIdentity {
                shared: crate::connectivity::core::context::ImsIdentity {
                    private_user: "502121234567890@ims.mnc012.mcc502.3gppnetwork.org".to_string(),
                    public_uri: "sip:502121234567890@ims.mnc012.mcc502.3gppnetwork.org".to_string(),
                    contact_user: "502121234567890".to_string(),
                    home_domain: "ims.mnc012.mcc502.3gppnetwork.org".to_string(),
                    contact_user_phone: false,
                },
                shape: "maxis_derived_fixture",
            },
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");

        let base_request = context.build_initial_request(profile, base);
        let declared_request = context.build_initial_request(profile, declared);
        let cumulative_request = context.build_initial_request(profile, cumulative);
        assert!(!base_request.contains("Authorization: Digest"));
        assert!(!base_request.contains("Security-Client:"));
        assert!(!base_request.contains("Require: sec-agree"));
        assert!(!declared_request.contains("Authorization: Digest"));
        for header in [
            "Security-Client:",
            "Require: sec-agree",
            "Proxy-Require: sec-agree",
        ] {
            assert!(
                declared_request.contains(header),
                "declared request missing {header}"
            );
            assert!(
                cumulative_request.contains(header),
                "cumulative request missing {header}"
            );
        }
        assert!(cumulative_request
            .contains("Authorization: Digest uri=\"sip:ims.mnc012.mcc502.3gppnetwork.org\""));
        assert!(cumulative_request
            .contains("username=\"502121234567890@ims.mnc012.mcc502.3gppnetwork.org\""));
        assert!(cumulative_request.contains("nonce=\"\""));
        assert!(cumulative_request.contains("response=\"\""));
        assert!(cumulative_request.contains("algorithm=AKAv1-MD5"));
        assert_eq!(
            cumulative_request.lines().next(),
            base_request.lines().next(),
            "dynamic upgrades must not change the REGISTER request URI"
        );
        assert_eq!(
            cumulative_request.contains("Route:"),
            base_request.contains("Route:"),
            "dynamic upgrades must preserve the Route policy"
        );
        assert!(cumulative_request.contains("P-Access-Network-Info: IEEE-802.11\r\n"));
        assert!(cumulative_request.contains("P-Preferred-Identity:"));

        let authenticated = context.build_authenticated_request(
            profile,
            cumulative,
            "Authorization: Digest username=\"impi\",realm=\"ims\",nonce=\"n\",uri=\"sip:ims\",response=\"proof\",algorithm=AKAv1-MD5",
            None,
        );
        for header in [
            "Authorization: Digest",
            "Security-Client:",
            "Require: sec-agree",
            "Proxy-Require: sec-agree",
        ] {
            assert!(
                authenticated.contains(header),
                "authenticated request missing {header}"
            );
        }
        assert!(
            !authenticated.contains("Security-Verify:"),
            "a 401 without Security-Server must not fabricate Security-Verify"
        );

        let compact = next_dynamic_live_register_variant(profile, cumulative, &bad_request)
            .expect("second 400 must retain AKA/sec-agree and compact Security-Client");
        assert_eq!(
            compact.initial_authorization,
            cumulative.initial_authorization
        );
        assert!(compact.server_required_sec_agree);
        assert_eq!(
            compact.security_client_format,
            LiveSecurityClientFormat::FullCompact
        );
        let minimal = next_dynamic_live_register_variant(profile, compact, &bad_request)
            .expect("third 400 must retain AKA/sec-agree and use the final minimal offer");
        assert_eq!(
            minimal.initial_authorization,
            cumulative.initial_authorization
        );
        assert!(minimal.server_required_sec_agree);
        assert_eq!(
            minimal.security_client_format,
            LiveSecurityClientFormat::MinimalSpaced
        );
        assert!(
            next_dynamic_live_register_variant(profile, minimal, &bad_request).is_none(),
            "the final response-driven shape must not cycle"
        );

        let after_auth = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            1,
        ));
        assert!(next_dynamic_live_register_variant(profile, declared, &after_auth).is_none());

        let mut disabled_profile = *profile;
        disabled_profile.ims.register.sec_agree_mode = "disabled";
        assert!(
            next_dynamic_live_register_variant(&disabled_profile, base, &requires_sec_agree)
                .is_none()
        );

        let mut explicit_authorization = declared;
        explicit_authorization.initial_authorization = LiveInitialAuthorizationFormat::AkaEmpty;
        let preserved =
            next_dynamic_live_register_variant(profile, explicit_authorization, &bad_request)
                .expect("an explicit Authorization shape may advance formatting only");
        assert_eq!(
            preserved.initial_authorization,
            LiveInitialAuthorizationFormat::AkaEmpty,
            "the ladder must not overwrite an explicitly selected Authorization format"
        );
        assert_eq!(
            preserved.security_client_format,
            LiveSecurityClientFormat::FullCompact
        );

        let mut profile_required = *profile;
        profile_required.ims.register.sec_agree_mode = "required";
        let profile_required = Box::leak(Box::new(profile_required));
        let explicit_required = live_register_header_variants(profile_required)[0];
        assert!(!explicit_required.server_required_sec_agree);
        assert!(
            next_dynamic_live_register_variant(profile_required, explicit_required, &bad_request)
                .is_none(),
            "a profile-level required policy alone is not evidence to rewrite Authorization"
        );

        // One base attempt plus the four monotonic transitions above remains
        // below the global safety budget.
        assert!(5 <= LIVE_IMS_REGISTER_MAX_VARIANT_ATTEMPTS);
    }

    #[test]
    fn explicit_sec_agree_disabled_is_never_upgraded_by_421_or_494() {
        let mut profile = GB_EE_23433;
        profile.ims.register.sec_agree_mode = "disabled";
        profile.ims.register.require_sec_agree_headers = false;
        profile.ims.register.proxy_require_sec_agree_headers = false;
        let variant = register_variant("ims_features_aka_uri_first_full_sec_client");
        for response in [
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            "SIP/2.0 494 Security Agreement Required\r\nContent-Length: 0\r\n\r\n",
        ] {
            let error = map_shared_register_failure(&register_failure_with(response, 0));
            assert!(error.server_required_sec_agree);
            assert!(sec_agree_retry_variant(&profile, variant, &error).is_none());
        }
    }

    #[test]
    fn sec_agree_421_upgrade_does_not_loop_or_fire_on_other_rejections() {
        let sec_agree_421 = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        // Already declaring sec-agree: a second identical attempt would loop.
        let mut forcing = register_variant("ims_features_aka_uri_first_full_sec_client");
        forcing.force_sec_agree_headers = true;
        assert!(sec_agree_retry_variant(&GB_EE_23433, forcing, &sec_agree_421).is_none());

        // A 421 naming a different extension is not ours to satisfy.
        let other_ext = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: timer\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        assert!(!other_ext.server_required_sec_agree);

        // A plain rejection carries no demand.
        let forbidden = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        assert!(!forbidden.server_required_sec_agree);

        // After authentication the security agreement is already settled.
        let after_auth = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            1,
        ));
        assert!(!after_auth.server_required_sec_agree);
    }

    #[test]
    fn terminal_register_status_stops_the_candidate_ladder() {
        let terminal = [
            300, 302, 403, 405, 406, 409, 413, 414, 416, 422, 432, 433, 436, 437, 438, 481, 482,
            483, 484, 485, 486, 487, 488, 489, 493, 505, 513, 580, 600, 603,
        ];
        for status in terminal {
            let failure = register_failure_with(
                &format!("SIP/2.0 {status} X\r\nContent-Length: 0\r\n\r\n"),
                0,
            );
            let error = map_shared_register_failure(&failure);
            assert!(
                live_register_error_is_terminal(&error),
                "status {status} should abort the REGISTER ladder"
            );
        }
    }

    #[test]
    fn retryable_register_status_keeps_the_candidate_ladder_alive() {
        let retryable = [
            400, 404, 408, 410, 415, 420, 421, 423, 430, 480, 491, 494, 500, 501, 502, 503, 504,
        ];
        for status in retryable {
            let failure = register_failure_with(
                &format!("SIP/2.0 {status} X\r\nContent-Length: 0\r\n\r\n"),
                0,
            );
            let error = map_shared_register_failure(&failure);
            assert!(
                !live_register_error_is_terminal(&error),
                "status {status} should keep trying further REGISTER candidates"
            );
        }
    }

    #[test]
    fn auth_rejected_without_status_is_terminal() {
        let failure = RegisterFailure {
            error: ImsError::new("ims_register_auth_rejected"),
            response: None,
            auth_rounds: 1,
        };
        let error = map_shared_register_failure(&failure);
        assert!(live_register_error_is_terminal(&error));
        assert!(live_register_error_status(&error).is_none());
    }

    fn ee_register_variant(label: &str) -> LiveRegisterHeaderVariant {
        *GB_EE_REGISTER_HEADER_VARIANTS
            .iter()
            .find(|variant| variant.label == label)
            .expect("EE register variant exists")
    }
    use crate::connectivity::modems::ims::vowifi::{
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
    fn generic_catalog_variants_keep_exact_policy_first_without_implicit_ipcc() {
        let mut profile = GB_EE_23433;
        profile.ims.register.live_header_variant_set = "catalog_v7";
        profile.ims.register.enable_initial_reject_fallback = true;
        profile.ims.register.include_pani_initial = false;
        profile.ims.register.include_pani_authenticated = false;
        profile.ims.register.enable_cellular_network_info = false;
        profile.ims.register.include_mmtel_features = true;
        let profile = Box::leak(Box::new(profile));

        let variants = live_register_header_variants(profile);
        assert_eq!(variants[0].label, "catalog_v7");
        assert!(!variants
            .iter()
            .any(|variant| variant.label == "catalog_v7_ipcc_access_baseline"));
        assert!(matches!(
            variants[0].header_profile.pani,
            LivePaniFormat::Omit
        ));
        assert!(!variants[0].header_profile.include_cellular_network_info);
        assert!(matches!(
            variants[0].header_profile.contact_features,
            LiveContactFeatureSet::MmtelSmsSipInstance
        ));
    }

    #[test]
    fn named_ipcc_fallback_is_after_exact_and_respects_access_header_disables() {
        let mut profile = GB_EE_23433;
        profile.ims.register.live_header_variant_set = "catalog_v7_ipcc_access_fallback";
        profile.ims.register.include_pani_initial = false;
        profile.ims.register.include_pani_authenticated = false;
        profile.ims.register.enable_cellular_network_info = false;
        profile.ims.register.include_mmtel_features = true;
        let profile = Box::leak(Box::new(profile));

        let variants = live_register_header_variants(profile);
        assert_eq!(variants[0].label, "catalog_v7_ipcc_access_fallback");
        let fallback = variants
            .iter()
            .skip(1)
            .find(|variant| variant.label == "catalog_v7_ipcc_access_baseline")
            .expect("named IPCC access fallback");
        assert!(matches!(fallback.header_profile.pani, LivePaniFormat::Omit));
        assert!(!fallback.header_profile.include_cellular_network_info);
        assert!(matches!(
            fallback.header_profile.contact_features,
            LiveContactFeatureSet::SmsOnly
        ));
    }

    #[test]
    fn live_runtime_config_defaults_to_shared_transport_environment() {
        let config = config_from_pairs(&[]);

        assert_eq!(config.qmi_proxy_socket, DEFAULT_QMI_PROXY_SOCKET);
        assert_eq!(config.tun_name, DEFAULT_LIVE_TUN_NAME);
        assert_eq!(config.ims_security_port_c, LIVE_IMS_SECURITY_PORT_C);
        assert_eq!(config.ims_security_port_s, LIVE_IMS_SECURITY_PORT_S);
    }

    #[test]
    fn live_runtime_config_accepts_non_sensitive_env_overrides() {
        let config = config_from_pairs(&[
            (ENV_QMI_PROXY_SOCKET, "@alt-qmi-proxy"),
            (ENV_TUN_NAME, "sa_vwf1"),
            (ENV_IMS_SECURITY_PORT_C, "6064"),
            (ENV_IMS_SECURITY_PORT_S, "6063"),
        ]);

        assert_eq!(config.qmi_proxy_socket, "@alt-qmi-proxy");
        assert_eq!(config.tun_name, "sa_vwf1");
        assert_eq!(config.ims_security_port_c, 6064);
        assert_eq!(config.ims_security_port_s, 6063);
    }

    #[test]
    fn live_runtime_config_rejects_empty_or_zero_overrides() {
        let config = config_from_pairs(&[
            (ENV_QMI_PROXY_SOCKET, " "),
            (ENV_TUN_NAME, " "),
            (ENV_IMS_SECURITY_PORT_C, "0"),
            (ENV_IMS_SECURITY_PORT_S, "not-a-port"),
        ]);

        assert_eq!(config.qmi_proxy_socket, DEFAULT_QMI_PROXY_SOCKET);
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
        let aka = crate::connectivity::modems::ims::vowifi::qmi_uim::UsimAkaApduResult {
            res: vec![0x22; 8],
            ck: vec![0x33; 16],
            ik: vec![0x44; 16],
            auts: None,
        };
        let proof = compute_aka_digest_response(
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
    fn akav2_sha256_challenge_is_accepted_and_emits_a_sha256_proof() {
        let nonce = BASE64_STANDARD.encode([0x11u8; 32]);
        let response = format!(
            concat!(
                "SIP/2.0 401 Unauthorized\r\n",
                "WWW-Authenticate: Digest realm=\"{}\", algorithm=AKAv2-SHA-256, nonce=\"{}\", qop=\"auth\"\r\n",
                "Content-Length: 0\r\n\r\n"
            ),
            GB_EE_23433.ims.realm, nonce
        );
        let challenge = parse_live_digest_challenge(&response, GB_EE_23433.ims.realm)
            .expect("parse AKAv2-SHA-256 challenge");
        let aka = crate::connectivity::modems::ims::vowifi::qmi_uim::UsimAkaApduResult {
            res: vec![0x22; 8],
            ck: vec![0x33; 16],
            ik: vec![0x44; 16],
            auts: None,
        };
        let proof = compute_aka_digest_response(
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
        .expect("compute AKAv2-SHA-256 proof");

        assert_eq!(challenge.algorithm, "AKAv2-SHA-256");
        assert_eq!(proof.len(), 64);
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
        let aka = crate::connectivity::modems::ims::vowifi::qmi_uim::UsimAkaApduResult {
            res: vec![0x66; 8],
            ck: vec![0x77; 16],
            ik: vec![0x88; 16],
            auts: None,
        };
        let proof = compute_aka_digest_response(
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
        assert!(initial.contains("+sip.instance="));
        assert!(!initial.contains(";reg-id="));
        assert!(initial.contains("P-Access-Network-Info: IEEE-802.11\r\n"));
        assert!(!initial.contains("i-wlan-node-id"));

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
    fn vowifi_cni_requires_profile_opt_in_and_a_real_cell_snapshot() {
        let mut configured = GB_EE_23433;
        configured.ims.register.enable_cellular_network_info = true;
        configured.ims.register.cni_identity_policy = AccessIdentityPolicy::DynamicIfKnown;
        let profile = Box::leak(Box::new(configured));
        let mut variant = register_variant("profile_default_spaced_sec_client");
        variant.header_profile.include_cellular_network_info = true;
        let mut context = LiveRegisterRequestContext::new(
            profile,
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

        let no_snapshot = context.build_initial_request(profile, variant);
        assert!(!no_snapshot.contains("Cellular-Network-Info:"));

        context.access_network = ImsAccessNetworkContext::new(
            crate::connectivity::core::access_network::ImsAccessType::EutranFdd,
            "50212",
            0x1234567,
            0x00ab,
            Some(0),
            crate::connectivity::core::access_network::AccessNetworkSource::TestFixture,
        );
        let with_snapshot = context.build_initial_request(profile, variant);
        assert!(with_snapshot.contains("P-Access-Network-Info: IEEE-802.11\r\n"));
        assert!(with_snapshot.contains(concat!(
            "Cellular-Network-Info: ",
            "3GPP-E-UTRAN-FDD;utran-cell-id-3gpp=5021200AB1234567;",
            "cell-info-age=0\r\n"
        )));

        let mut auth_phase_only = *profile;
        auth_phase_only.ims.register.include_pani_initial = false;
        auth_phase_only.ims.register.include_pani_authenticated = true;
        let auth_phase_only = Box::leak(Box::new(auth_phase_only));
        let initial_without_pani = context.build_initial_request(auth_phase_only, variant);
        assert!(!initial_without_pani.contains("P-Access-Network-Info:"));
        assert!(
            !initial_without_pani.contains("Cellular-Network-Info:"),
            "CNI must not be emitted in a phase that intentionally omits PANI"
        );
        let authenticated_with_pani = context.build_authenticated_request(
            auth_phase_only,
            variant,
            "Authorization: Digest username=\"redacted\",realm=\"ims.example\",nonce=\"redacted\",uri=\"sip:ims.example\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
            None,
        );
        assert!(authenticated_with_pani.contains("P-Access-Network-Info: IEEE-802.11\r\n"));
        assert!(authenticated_with_pani.contains("Cellular-Network-Info:"));

        let mut omit_pani_variant = variant;
        omit_pani_variant.header_profile.pani = LivePaniFormat::Omit;
        let omitted_by_variant = context.build_initial_request(profile, omit_pani_variant);
        assert!(!omitted_by_variant.contains("P-Access-Network-Info:"));
        assert!(
            !omitted_by_variant.contains("Cellular-Network-Info:"),
            "CNI must not bypass a variant that explicitly omits PANI"
        );

        let mut disabled = *profile;
        disabled.ims.register.enable_cellular_network_info = false;
        let disabled = Box::leak(Box::new(disabled));
        let explicitly_disabled = context.build_initial_request(disabled, variant);
        assert!(
            !explicitly_disabled.contains("Cellular-Network-Info:"),
            "database/catalog false must suppress CNI even when a snapshot exists"
        );
    }

    #[test]
    fn contact_access_type_does_not_copy_pani_parameters() {
        let mut configured = GB_EE_23433;
        configured.ims.register.access_network_info =
            "IEEE-802.11;i-wlan-node-id=operator-specific";
        let profile = Box::leak(Box::new(configured));
        let context = LiveRegisterRequestContext::new(
            profile,
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
            profile,
            register_variant("profile_default_spaced_sec_client"),
        );
        assert!(request
            .contains("P-Access-Network-Info: IEEE-802.11;i-wlan-node-id=operator-specific\r\n"));
        assert!(request.contains("+g.3gpp.accesstype=\"IEEE-802.11\""));
        assert!(!request
            .contains("+g.3gpp.accesstype=\"IEEE-802.11;i-wlan-node-id=operator-specific\""));
    }

    #[test]
    fn unregister_factory_reuses_registered_dialog_and_zeroes_expiry() {
        let mut context = LiveRegisterRequestContext::new(
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
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5064),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        )
        .expect("register context");
        context.from_tag = "registered-from-tag".into();
        context.call_id = "registered-call-id@simadmin".into();
        context.protected_header_port = Some(5063);
        let factory = VowifiUnregisterFactory {
            line_id: "line-unregister".into(),
            profile: &GB_EE_23433,
            context,
            variant: register_variant("profile_default_spaced_sec_client"),
            next_cseq: 4,
            security_verify: Some(
                "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans".into(),
            ),
        };

        let request = String::from_utf8(
            crate::connectivity::modems::ims::vowifi::operator::RegisteredUnregister::initial_request(
                &factory,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(request.starts_with("REGISTER "));
        assert!(
            request.contains("From: <sip:001010123456789@ims.example>;tag=registered-from-tag\r\n")
        );
        assert!(request.contains("Call-ID: registered-call-id@simadmin\r\n"));
        assert!(request.contains("CSeq: 4 REGISTER\r\n"));
        assert!(request.contains("Expires: 0\r\n"));
        assert!(request.contains("Security-Verify: ipsec-3gpp;"));
        assert!(request.contains("Via: SIP/2.0/"));
        assert!(request.contains("[::1]:5063;branch="));
    }

    #[test]
    fn vowifi_catalog_video_contact_feature_requires_local_capability() {
        let mut profile = GB_EE_23433;
        profile.ims.register.contact_param_order = &["audio", "video", "+g.3gpp.smsip"];
        let profile = Box::leak(Box::new(profile));
        let identity = || LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: "001010123456789@ims.example".to_string(),
                public_uri: "sip:001010123456789@ims.example".to_string(),
                contact_user: "001010123456789".to_string(),
                home_domain: "ims.example".to_string(),
                contact_user_phone: false,
            },
            shape: "fixture",
        };
        let build = |video_capability_enabled| {
            LiveRegisterRequestContext::new_with_target_and_device(
                profile,
                live_ims_target("", profile),
                identity(),
                SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5060),
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                None,
                video_capability_enabled,
            )
            .unwrap()
            .build_initial_request(
                profile,
                register_variant("profile_default_spaced_sec_client"),
            )
        };
        let disabled = build(false);
        assert!(disabled.contains(";audio"));
        assert!(!disabled.contains(";video"));
        let enabled = build(true);
        assert!(enabled.contains(";audio;video;+g.3gpp.smsip"));
        assert_eq!(
            enabled
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn vowifi_catalog_register_adds_missing_sms_over_ip_feature_tag() {
        let mut profile = GB_EE_23433;
        profile.ims.register.contact_param_order = &["+g.3gpp.mid-call"];
        let profile = Box::leak(Box::new(profile));
        let context = LiveRegisterRequestContext::new_with_target_and_device(
            profile,
            live_ims_target("", profile),
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
            None,
            false,
        )
        .unwrap();
        let request = context.build_initial_request(
            profile,
            register_variant("profile_default_spaced_sec_client"),
        );
        assert!(request.contains(";+g.3gpp.mid-call"));
        assert!(request.contains(";+g.3gpp.smsip"));
        assert_eq!(
            request
                .to_ascii_lowercase()
                .matches("+g.3gpp.smsip")
                .count(),
            1
        );
    }

    #[test]
    fn empty_aka_initial_register_does_not_force_security_client() {
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
        let variant = register_variant("ims_features_aka_empty_no_security_client");

        let initial = context.build_initial_request(&GB_EE_23433, variant);
        assert!(initial.contains("Authorization: Digest"));
        assert!(initial.contains("response=\"\""));
        assert!(!initial.contains("Security-Client:"));
        assert!(!initial.contains("Require: sec-agree"));
        assert!(!initial.contains("Proxy-Require: sec-agree"));

        let authenticated = context.build_authenticated_request(
            &GB_EE_23433,
            variant,
            "Authorization: Digest username=\"x\",realm=\"r\",nonce=\"n\",uri=\"sip:r\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
            None,
        );
        assert!(!authenticated.contains("Security-Client:"));
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
    fn protected_udp_register_advertises_protected_server_port_in_via_and_contact() {
        let mut context = LiveRegisterRequestContext::new(
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
        // The protected round sources the packet from port_uc (5064), but
        // TS 24.229 §5.1.1.2.2 requires Via/Contact to advertise port_us
        // (5063) for UDP.
        context.local_addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 5064);
        context.transport = crate::connectivity::core::context::SipTransport::Udp;
        context.protected_header_port = Some(5063);
        let variant = register_variant("ims_features_plain_pani");

        let authenticated = context.build_authenticated_request(
            &GB_EE_23433,
            variant,
            "Authorization: Digest username=\"x\",realm=\"r\",nonce=\"n\",uri=\"sip:r\",response=\"00000000000000000000000000000000\",algorithm=AKAv1-MD5",
            Some("ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans"),
        );

        assert!(
            authenticated.contains("Via: SIP/2.0/UDP [::1]:5063;branch="),
            "protected UDP Via must advertise port_us, got: {}",
            authenticated.lines().next().unwrap_or_default()
        );
        assert!(
            authenticated.contains("Contact: <sip:001010123456789@[::1]:5063;"),
            "protected UDP Contact must advertise port_us"
        );
        assert!(authenticated.contains("Security-Verify: ipsec-3gpp;"));
    }

    #[test]
    fn wlan_reg_id_never_collides_with_the_cellular_leg() {
        // Both legs now present one stable +sip.instance (so the instance id
        // names the UE, per RFC 5626 §4.1). A binding is keyed on
        // (AOR, instance-id, reg-id) by §6, so equal reg-ids would make
        // whichever leg registers second *replace* the other's binding (§3.2)
        // while this runtime still reported both as registered. Guard the two
        // constants against drifting onto the same value.
        use crate::connectivity::core::ims_access::ImsAccess;
        assert_eq!(WLAN_REG_ID, ImsAccess::Wlan.reg_id());
        assert_ne!(WLAN_REG_ID, ImsAccess::Cellular.reg_id());
    }

    #[test]
    fn unprotected_register_keeps_normal_sip_port_in_via_and_contact() {
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
        let variant = register_variant("ims_features_plain_pani");

        let initial = context.build_initial_request(&GB_EE_23433, variant);
        assert!(initial.contains("Via: SIP/2.0/TCP [::1]:5060;branch="));
        assert!(initial.contains("Contact: <sip:001010123456789@[::1]:5060;"));
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
        assert!(!initial.contains("; mod="));
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
            &crate::connectivity::modems::ims::vowifi::profiles::NL_VODAFONE_20404,
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
            &crate::connectivity::modems::ims::vowifi::profiles::NL_VODAFONE_20404,
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
        assert!(!forced.contains("Proxy-Require: sec-agree\r\n"));
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
                server_required_sec_agree: false,
                suppress_sec_agree_headers: false,
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

        let addrs = pcscf_candidates("", &GB_EE_23433, &configuration, inner);

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
    fn vowifi_register_errors_map_to_shared_loss_reasons() {
        for reason in [
            "ims_aka_runtime_failed",
            "ims_digest_nonce_missing",
            "eap_aka_challenge_parse_failed",
            "sim_auth_runtime_failed",
        ] {
            assert_eq!(
                classify_vowifi_register_error(reason),
                RegistrationLossReason::AuthenticationRejected,
                "{reason}"
            );
        }
        for reason in [
            "ims_tcp_connect_timeout",
            "ims_register_read_failed",
            "ims_udp_bind_failed",
        ] {
            assert_eq!(
                classify_vowifi_register_error(reason),
                RegistrationLossReason::SignalingTransportLost,
                "{reason}"
            );
        }
        assert_eq!(
            classify_vowifi_register_error("ims_security_server_offer_unmatched"),
            RegistrationLossReason::NetworkRejected
        );
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
    fn security_server_offer_requires_explicit_algorithms_and_ports() {
        let missing_algorithms = "ipsec-3gpp;spi-c=1;spi-s=2;port-c=5062;port-s=5064";
        assert_eq!(
            parse_live_security_server_offer(missing_algorithms)
                .expect_err("algorithms must not be defaulted")
                .reason,
            "ims_security_server_parameter_missing"
        );

        let missing_ports =
            "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=1;spi-s=2";
        assert_eq!(
            parse_live_security_server_offer(missing_ports)
                .expect_err("ports must not be defaulted")
                .reason,
            "ims_security_server_port_missing"
        );
    }

    #[tokio::test]
    async fn clearing_one_line_keeps_another_lines_register_variant() {
        let line_a = "line-register-variant-a";
        let line_b = "line-register-variant-b";
        let success = ee_register_variant("gb_ee_aka_uri_first_required_sec_agree");

        record_live_ims_register_success_variant(line_a, &GB_EE_23433, success).await;
        record_live_ims_register_success_variant(line_b, &GB_EE_23433, success).await;
        clear_live_runtime_for_line(line_a).await;
        let variants = live_register_header_variants_for_attempt(line_b, &GB_EE_23433).await;

        assert_eq!(
            variants.first().map(|variant| variant.label),
            Some("gb_ee_aka_uri_first_sec_client")
        );
        assert_eq!(
            variants.get(1).map(|variant| variant.label),
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
        clear_live_runtime_for_line(line_b).await;
    }

    #[tokio::test]
    async fn response_driven_register_success_cache_preserves_the_full_shape() {
        let line_id = "line-register-dynamic-variant-cache";
        let profile =
            crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
                "502",
                "12",
                crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::WifiEpdg,
            )
            .expect("derived Maxis Wi-Fi profile");
        ims_register_variant_cache().lock().await.remove(line_id);

        let base = live_register_header_variants(profile)[0];
        let requires_sec_agree = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        let declared = next_dynamic_live_register_variant(profile, base, &requires_sec_agree)
            .expect("421 upgrade");
        let bad_request = map_shared_register_failure(&register_failure_with(
            "SIP/2.0 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            0,
        ));
        let success = next_dynamic_live_register_variant(profile, declared, &bad_request)
            .expect("cumulative empty-AKA upgrade");
        record_live_ims_register_success_variant(line_id, profile, success).await;

        let variants = live_register_header_variants_for_attempt(line_id, profile).await;
        assert_eq!(
            variants.first().map(|variant| variant.label),
            Some(base.label)
        );
        let cached = variants
            .get(1)
            .expect("dynamic success is preferred after exact policy");
        assert_eq!(cached.label, success.label);
        assert!(cached.force_sec_agree_headers);
        assert!(cached.server_required_sec_agree);
        assert!(cached.include_security_client);
        assert_eq!(
            cached.initial_authorization,
            LiveInitialAuthorizationFormat::AkaEmptyUriFirst
        );
        assert_eq!(
            variants.len(),
            live_register_header_variants(profile).len() + 1
        );

        // Reusing a profile ID after a database edit must not replay a request
        // shape captured from the previous immutable profile object.
        let reloaded_profile = Box::leak(Box::new(*profile));
        let after_reload =
            live_register_header_variants_for_attempt(line_id, reloaded_profile).await;
        assert_eq!(
            after_reload.len(),
            live_register_header_variants(reloaded_profile).len()
        );
        assert!(after_reload
            .iter()
            .all(|variant| variant.label != success.label));

        ims_register_variant_cache().lock().await.remove(line_id);
    }

    #[test]
    fn sms_send_total_timeout_stays_inside_http_live_budget() {
        assert!(LIVE_SMS_SEND_TOTAL_TIMEOUT < Duration::from_secs(90));
        assert!(
            LIVE_SMS_SEND_TOTAL_TIMEOUT >= LIVE_IMS_TCP_TIMEOUT + LIVE_IMS_REGISTER_READ_TIMEOUT
        );
    }

    #[test]
    fn operator_call_events_map_to_public_call_lifecycle() {
        let seed = voice::MoCallSipOutcome {
            trace_id: "trace-a".into(),
            call_id: "call-a".into(),
            sip_status: 0,
            invite_state: voice::SipInviteState::Queued,
            call_state: voice::CallState::Dialing,
            negotiated_codec: None,
            failure_cause: None,
        };
        let offered = [voice::AudioCodec::AmrWb, voice::AudioCodec::Pcmu];

        let (ringing, terminal) = operator_event_call_outcome(
            &seed,
            &offered,
            &OperatorEvent::Provisional {
                call_id: "call-a".into(),
                status: 183,
                body: None,
            },
        )
        .expect("matching provisional event");
        assert!(!terminal);
        assert_eq!(ringing.sip_status, 183);
        assert_eq!(ringing.invite_state, voice::SipInviteState::EarlyMedia);
        assert_eq!(ringing.call_state, voice::CallState::Ringing);

        let answer = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 32000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
        let (answered, terminal) = operator_event_call_outcome(
            &seed,
            &offered,
            &OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: answer.to_vec(),
            },
        )
        .expect("matching answer event");
        assert!(!terminal);
        assert_eq!(answered.call_state, voice::CallState::Active);
        assert_eq!(answered.invite_state, voice::SipInviteState::Confirmed);
        assert_eq!(answered.negotiated_codec, Some(voice::AudioCodec::Pcmu));

        let (rejected, terminal) = operator_event_call_outcome(
            &seed,
            &offered,
            &OperatorEvent::Rejected {
                call_id: "call-a".into(),
                status: 486,
                diagnostic:
                    crate::connectivity::core::ims_failure::ImsFailureDiagnostic::from_status(486),
            },
        )
        .expect("matching rejection event");
        assert!(terminal);
        assert_eq!(rejected.call_state, voice::CallState::Failed);
        assert_eq!(rejected.failure_cause.as_deref(), Some("callee_busy"));

        assert!(operator_event_call_outcome(
            &seed,
            &offered,
            &OperatorEvent::Ended {
                call_id: "another-call".into(),
            },
        )
        .is_none());
    }

    #[tokio::test]
    async fn refresh_requeues_mwi_message_options_and_invite_for_session_loop() {
        let peer = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let local = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let local_addr = local.local_addr().unwrap();
        let peer_addr = peer.local_addr().unwrap();
        local.connect(peer_addr).await.unwrap();
        let route = crate::connectivity::core::context::ImsRoute {
            local_addr,
            pcscf_addr: peer_addr,
            transport: crate::connectivity::core::context::SipTransport::Udp,
        };
        let mut channel = SipChannel::new(SipChannelSocket::Udp(local), Vec::new(), route, None);

        let request =
            "REGISTER sip:ims.example SIP/2.0\r\nCall-ID: refresh@dev\r\nCSeq: 8 REGISTER\r\nContent-Length: 0\r\n\r\n";
        let frames: [&[u8]; 5] = [
            b"NOTIFY sip:user@ims.example SIP/2.0\r\nCall-ID: mwi@dev\r\nCSeq: 1 NOTIFY\r\nEvent: message-summary\r\nContent-Length: 0\r\n\r\n",
            b"MESSAGE sip:user@ims.example SIP/2.0\r\nCall-ID: message@dev\r\nCSeq: 1 MESSAGE\r\nContent-Length: 0\r\n\r\n",
            b"OPTIONS sip:user@ims.example SIP/2.0\r\nCall-ID: options@dev\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n",
            b"INVITE sip:user@ims.example SIP/2.0\r\nCall-ID: invite@dev\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n",
            b"SIP/2.0 200 OK\r\nCall-ID: refresh@dev\r\nCSeq: 8 REGISTER\r\nContact: <sip:user@192.0.2.2>;expires=120\r\nContent-Length: 0\r\n\r\n",
        ];
        for frame in frames {
            peer.send_to(frame, local_addr).await.unwrap();
        }

        let response = read_final_register_response_with_timeout(
            &mut channel,
            request,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(sip_frame::parse_status(response.as_bytes()).unwrap(), 200);

        for (method, expected_call_id) in [
            ("NOTIFY", "mwi@dev"),
            ("MESSAGE", "message@dev"),
            ("OPTIONS", "options@dev"),
            ("INVITE", "invite@dev"),
        ] {
            let frame = channel
                .recv_sip(Duration::from_secs(1))
                .await
                .expect("requeued SIP frame");
            assert!(sip_frame::is_request(&frame, method), "expected {method}");
            assert_eq!(
                sip_frame::header_value(&frame, "Call-ID").as_deref(),
                Some(expected_call_id)
            );
        }
    }

    #[tokio::test]
    async fn shared_register_contract_covers_vowifi_exchange_shape() {
        crate::connectivity::core::register::contract::assert_register_contract(
            crate::connectivity::core::register::contract::AuthenticatedExchangeStyle::AdapterOwned,
            ImsRegistrationAccess::Vowifi,
        )
        .await;
    }

    #[tokio::test]
    async fn adapter_owned_register_exchange_skips_provisional_frames() {
        let peer = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let local = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let local_addr = local.local_addr().unwrap();
        let peer_addr = peer.local_addr().unwrap();
        local.connect(peer_addr).await.unwrap();
        let route = crate::connectivity::core::context::ImsRoute {
            local_addr,
            pcscf_addr: peer_addr,
            transport: crate::connectivity::core::context::SipTransport::Udp,
        };
        let mut channel = SipChannel::new(SipChannelSocket::Udp(local), Vec::new(), route, None);

        let request = "REGISTER sip:ims.example SIP/2.0\r\nCall-ID: adapter@dev\r\nCSeq: 4 REGISTER\r\nContent-Length: 0\r\n\r\n";
        for frame in [
            b"SIP/2.0 100 Trying\r\nCall-ID: adapter@dev\r\nCSeq: 4 REGISTER\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"SIP/2.0 183 Session Progress\r\nCall-ID: adapter@dev\r\nCSeq: 4 REGISTER\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"SIP/2.0 200 OK\r\nCall-ID: adapter@dev\r\nCSeq: 4 REGISTER\r\nContact: <sip:user@192.0.2.2>;expires=120\r\nContent-Length: 0\r\n\r\n"
                .as_slice(),
        ] {
            peer.send_to(frame, local_addr).await.unwrap();
        }

        let response = read_final_register_response_with_timeout(
            &mut channel,
            request,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(sip_frame::parse_status(response.as_bytes()).unwrap(), 200);
        assert_eq!(
            RegisterArtifacts::parse(response.as_bytes()).expires_seconds,
            Some(120)
        );
    }
}
