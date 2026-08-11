//! QMI WDS session control: allocate one client id, then reuse it across the
//! several `qmicli` invocations an IMS bearer needs.
//!
//! # Why the client id has to be reused
//!
//! Bringing up an IMS bearer is not one command. It is
//! `--wds-set-ip-family` → `--wds-start-network` → `--wds-get-current-settings`,
//! and the last step is the one that carries the P-CSCF (delivered by the
//! network via PCO). `qmicli` accepts only a single WDS action per invocation
//! (`too many WDS actions requested` otherwise), and the session plus its
//! settings live on the WDS *client* — not on the device node. So every step has
//! to run on the same client id: allocated once with
//! `--wds-noop --client-no-release-cid`, then passed back as `--client-cid=<n>`.
//!
//! # Which endpoint can do that (measured on the reference device, 2026-07-27)
//!
//! | | `/dev/wwan0qmi0` via `qmi-proxy` | `/dev/wwan0qmi1` (rpmsg `DATA6_CNTL`) |
//! |---|---|---|
//! | one-shot `--wds-start-network` | works | works |
//! | reuse a CID in a later process | works | `Transaction timed out` |
//!
//! `qmi-proxy` multiplexes QMUX over the primary control port, so a CID
//! allocated by one process stays addressable by the next — this is also how
//! ModemManager itself reaches the modem (on the reference device the fd for
//! `/dev/wwan0qmi0` is held by `qmi-proxy`, not by ModemManager). A spare rpmsg
//! endpoint is a bare pipe: every `open()` is a fresh session, the previous CID
//! is unreachable, and a running proxy does not help.
//!
//! Consequence, and it is the opposite of this project's first assumption: the
//! **IMS flow belongs on the primary port**, while a spare endpoint can only
//! carry a *single-shot* session — good enough for a plain data bearer, useless
//! for IMS.
//!
//! # Commands that must never be sent
//!
//! `--wds-bind-data-port` and `--wds-bind-mux-data-port` are unsupported by the
//! 2015 MSM8916 firmware (`InvalidArgument` / `InvalidQmiCommand`), and issuing
//! one *poisons the client*: the next `--wds-start-network` on it fails with
//! `endpoint hangup` and the baseband subsystem restarts. That was reproduced
//! twice, once escalating from a subsystem restart to a full device reboot. No
//! function in this module emits a bind argument, and `bind_arguments_are_never_emitted`
//! guards against one being added.

use std::{process::Output, time::Duration};

use tokio::process::Command;
use tracing::{debug, warn};

/// Data-format flags a dedicated endpoint must be opened with.
///
/// This is not cosmetic: on the reference hardware, omitting it makes WDS client
/// allocation on a spare endpoint fail with `CID allocation failed in the CTL
/// client: endpoint hangup`, while including it allocates reliably.
/// `--wda-get-data-format` on the endpoint confirms why — it reports link-layer
/// `raw-ip` with no QoS header, so the client has to be opened the same way.
///
/// It is deliberately *not* passed on the primary port: there the link format is
/// already established by whoever owns the modem (ModemManager), and the proven
/// primary-port command sequence does not include it.
pub const QMI_OPEN_NET_ARG: &str = "--device-open-net=net-raw-ip|net-no-qos-header";

const CID_TIMEOUT: Duration = Duration::from_secs(20);
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
/// `--wds-start-network` waits on the network's PDP activation, so it needs more
/// headroom than a plain query.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// How a device must be opened by `qmicli`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QmiOpenMode {
    /// `--device-open-qmi`: force QMI on a port whose advertised type is wrong.
    ForceQmi,
    /// `--device-open-proxy`: share the port through `qmi-proxy`. Required for
    /// CID reuse, and the only safe way to touch a port someone else owns.
    Proxy,
}

impl QmiOpenMode {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::ForceQmi => "--device-open-qmi",
            Self::Proxy => "--device-open-proxy",
        }
    }

    /// Probe order. `ForceQmi` first because a dedicated secondary endpoint is
    /// held exclusively by us and may be mislabeled; `Proxy` covers USB stacks.
    pub fn probe_order() -> [Self; 2] {
        [Self::ForceQmi, Self::Proxy]
    }
}

/// A QMI endpoint plus the capabilities that were *measured* on it, rather than
/// inferred from its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WdsEndpoint {
    pub device_path: String,
    pub open_mode: QmiOpenMode,
    /// Pass [`QMI_OPEN_NET_ARG`] when opening.
    pub force_net_format: bool,
    /// Whether a CID allocated here is still addressable from the next process.
    /// Only then can a multi-step flow (i.e. IMS) run on this endpoint.
    pub cid_reuse: bool,
}

impl WdsEndpoint {
    /// The baseband's primary QMI control port, reached through `qmi-proxy`.
    ///
    /// This is the endpoint the IMS bearer runs on: the proxy makes CIDs
    /// survive across processes, and it is how the port is already being shared
    /// with ModemManager.
    pub fn primary_via_proxy(device_path: impl Into<String>) -> Self {
        Self {
            device_path: device_path.into(),
            open_mode: QmiOpenMode::Proxy,
            force_net_format: false,
            cid_reuse: true,
        }
    }

    /// A dedicated spare endpoint (rpmsg `DATA*_CNTL`). Single-shot only.
    pub fn secondary(device_path: impl Into<String>, open_mode: QmiOpenMode) -> Self {
        Self {
            device_path: device_path.into(),
            open_mode,
            force_net_format: true,
            cid_reuse: false,
        }
    }

    /// Whether the multi-step IMS flow can run here.
    pub fn supports_ims_flow(&self) -> bool {
        self.cid_reuse
    }

    fn open_args(&self) -> Vec<&str> {
        let mut args = vec!["-d", self.device_path.as_str(), self.open_mode.as_arg()];
        if self.force_net_format {
            args.push(QMI_OPEN_NET_ARG);
        }
        args
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WdsError {
    /// `qmicli` could not be run at all.
    SpawnFailed(String),
    /// The command did not answer within its timeout.
    Timeout(String),
    /// No client id could be allocated.
    CidAllocationFailed(String),
    /// The endpoint answered, but a CID allocated on it is not addressable from
    /// a later process — so no IMS flow is possible here.
    CidReuseUnsupported(String),
    /// The endpoint cannot carry a multi-step flow by construction.
    ImsFlowUnsupported(String),
    /// `--wds-start-network` failed. `reason` is the modem's own verbose call-end
    /// reason when it gave one; it distinguishes "the network wants the other
    /// family" from a real failure.
    StartFailed { reason: String },
    /// The baseband stopped accepting session setup. Retrying is actively
    /// harmful — it can escalate into a device-wide reset.
    BasebandWedged(String),
    /// The session came up but its settings could not be read back.
    SettingsUnavailable(String),
    /// The retained client answered, but did not report a packet-service state.
    PacketStatusUnavailable(String),
}

impl std::fmt::Display for WdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(d) => write!(f, "qmi_wds_spawn_failed:{d}"),
            Self::Timeout(d) => write!(f, "qmi_wds_timeout:{d}"),
            Self::CidAllocationFailed(d) => write!(f, "qmi_wds_cid_allocation_failed:{d}"),
            Self::CidReuseUnsupported(d) => write!(f, "qmi_wds_cid_reuse_unsupported:{d}"),
            Self::ImsFlowUnsupported(d) => write!(f, "qmi_wds_ims_flow_unsupported:{d}"),
            Self::StartFailed { reason } => write!(f, "qmi_wds_start_failed:{reason}"),
            Self::BasebandWedged(d) => write!(f, "qmi_wds_baseband_wedged:{d}"),
            Self::SettingsUnavailable(d) => write!(f, "qmi_wds_settings_unavailable:{d}"),
            Self::PacketStatusUnavailable(d) => {
                write!(f, "qmi_wds_packet_status_unavailable:{d}")
            }
        }
    }
}

impl std::error::Error for WdsError {}

impl WdsError {
    /// Whether it is unsafe to retry against the same baseband. Mirrors
    /// `connectivity::modems::ims::volte::plan::FailureClass::is_unsafe_to_retry`, which stays the
    /// authority for classifying ModemManager-level failures.
    pub fn is_unsafe_to_retry(&self) -> bool {
        matches!(self, Self::BasebandWedged(_))
    }
}

/// Signatures of a baseband that has stopped accepting session setup.
///
/// Kept in sync with `connectivity::modems::ims::volte::plan::is_baseband_wedge`; duplicated rather
/// than shared because the cellular layer must be able to abort a family loop
/// without depending on the VoLTE layer's error vocabulary.
fn is_wedge_signature(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("endpoint hangup")
        || text.contains("interface-in-use-config-match")
        || text.contains("mobileequipment.unknown")
}

/// A WDS client id held open across several `qmicli` invocations.
///
/// Dropping this leaks the CID until the modem reclaims it. Call
/// [`WdsClient::release`] when the session is finished; a live session keeps its
/// client deliberately, because stopping the session needs the same CID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WdsClient {
    endpoint: WdsEndpoint,
    cid: String,
}

impl WdsClient {
    /// Allocate a client id with a no-op, keeping it alive for later steps.
    ///
    /// `--wds-noop` is used precisely because it carries no business action: the
    /// point is only to get a CID that subsequent invocations can address.
    pub async fn allocate(endpoint: &WdsEndpoint) -> Result<Self, WdsError> {
        if !endpoint.supports_ims_flow() {
            return Err(WdsError::ImsFlowUnsupported(format!(
                "{} cannot keep a CID across processes",
                endpoint.device_path
            )));
        }
        let mut args = endpoint.open_args();
        args.extend_from_slice(&["--client-no-release-cid", "--wds-noop"]);
        let text = capture(&args, CID_TIMEOUT).await?;
        let cid = parse_retained_cid(&text).ok_or_else(|| {
            if is_wedge_signature(&text) {
                WdsError::BasebandWedged(first_error_line(&text))
            } else {
                WdsError::CidAllocationFailed(first_error_line(&text))
            }
        })?;
        debug!(device = %endpoint.device_path, cid = %cid, "Allocated a retained WDS client");
        Ok(Self {
            endpoint: endpoint.clone(),
            cid,
        })
    }

    pub fn cid(&self) -> &str {
        &self.cid
    }

    pub fn endpoint(&self) -> &WdsEndpoint {
        &self.endpoint
    }

    /// Run one WDS action on this client, keeping the CID for the next step.
    async fn invoke(&self, action: &str, timeout: Duration) -> Result<String, WdsError> {
        let mut args = self.endpoint.open_args();
        let cid_arg = format!("--client-cid={}", self.cid);
        args.extend_from_slice(&[cid_arg.as_str(), "--client-no-release-cid", action]);
        let text = capture(&args, timeout).await?;
        // A bare rpmsg endpoint answers a reused CID with a transaction timeout
        // instead of an error, which is the fingerprint of "no QMUX multiplexing
        // here" rather than of a bad request.
        if text.to_ascii_lowercase().contains("transaction timed out") {
            return Err(WdsError::CidReuseUnsupported(format!(
                "{} lost CID {}",
                self.endpoint.device_path, self.cid
            )));
        }
        if is_wedge_signature(&text) {
            return Err(WdsError::BasebandWedged(first_error_line(&text)));
        }
        Ok(text)
    }

    /// Preselect the address family for the session. Sent as its own invocation
    /// because `qmicli` allows only one WDS action at a time.
    pub async fn set_ip_family(&self, family: u8) -> Result<(), WdsError> {
        self.invoke(&format!("--wds-set-ip-family={family}"), QUERY_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// Start the session and return its packet data handle.
    pub async fn start_network(
        &self,
        apn: &str,
        family: u8,
        profile_id: Option<u32>,
    ) -> Result<String, WdsError> {
        let mut action = format!("--wds-start-network=apn={apn}");
        if let Some(profile) = profile_id {
            action.push_str(&format!(",3gpp-profile={profile}"));
        }
        action.push_str(&format!(",ip-type={family}"));
        let text = self.invoke(&action, START_TIMEOUT).await?;
        match parse_packet_data_handle(&text) {
            Some(handle) => Ok(handle),
            None => Err(WdsError::StartFailed {
                reason: parse_call_end_reason(&text).unwrap_or_else(|| first_error_line(&text)),
            }),
        }
    }

    /// Read back the session's IP configuration — and the P-CSCF, which the
    /// network delivers here via PCO. This is the step that makes CID reuse a
    /// hard requirement.
    pub async fn current_settings(&self) -> Result<CurrentSettings, WdsError> {
        let text = self
            .invoke("--wds-get-current-settings", QUERY_TIMEOUT)
            .await?;
        let settings = parse_current_settings(&text);
        if settings.is_empty() {
            return Err(WdsError::SettingsUnavailable(first_error_line(&text)));
        }
        Ok(settings)
    }

    /// Read whether the retained WDS session is still connected.
    ///
    /// This is deliberately queried through the same CID as start/settings. A
    /// device-wide status query could describe another bearer and leave a dead
    /// IMS session looking healthy on a multi-PDN modem.
    pub async fn packet_service_status(&self) -> Result<PacketServiceStatus, WdsError> {
        let text = self
            .invoke("--wds-get-packet-service-status", QUERY_TIMEOUT)
            .await?;
        parse_packet_service_status(&text)
            .ok_or_else(|| WdsError::PacketStatusUnavailable(first_error_line(&text)))
    }

    pub async fn stop_network(&self, handle: &str) -> Result<(), WdsError> {
        self.invoke(&format!("--wds-stop-network={handle}"), STOP_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// Give the client id back to the modem: same invocation shape, minus
    /// `--client-no-release-cid`, so `qmicli` releases it on exit.
    pub async fn release(self) {
        let mut args = self.endpoint.open_args();
        let cid_arg = format!("--client-cid={}", self.cid);
        args.extend_from_slice(&[cid_arg.as_str(), "--wds-noop"]);
        if let Err(error) = capture(&args, QUERY_TIMEOUT).await {
            debug!(cid = %self.cid, error = %error, "Releasing the WDS client failed");
        }
    }
}

/// An established IMS session on a QMI endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsSession {
    /// Client id the session lives on. Needed to stop it, so it is held for the
    /// lifetime of the session rather than released after setup.
    pub client: WdsClient,
    /// WDS packet data handle.
    pub packet_data_handle: String,
    /// Family that was actually established (`4` or `6`).
    pub ip_family: u8,
    pub settings: CurrentSettings,
}

/// Bring up the IMS bearer, trying `families` in order.
///
/// The order comes from the caller's connection plan. A network that refuses one
/// family says so in the verbose call-end reason (`[3gpp] ipv4-only-allowed` on
/// the reference SIM), which is surfaced so the caller can see why a family was
/// skipped. A wedge signature aborts immediately instead of moving on: hammering
/// a wedged baseband with more PDP activations is what escalates a subsystem
/// restart into a dead device.
pub async fn start_ims_session(
    endpoint: &WdsEndpoint,
    apn: &str,
    families: &[u8],
    profile_id: Option<u32>,
) -> Result<ImsSession, WdsError> {
    let client = WdsClient::allocate(endpoint).await?;
    let mut last_error = None;
    for family in families.iter().copied() {
        // Best-effort: some firmware answers this with an error yet still honors
        // the ip-type on start-network, so it must not fail the whole attempt.
        if let Err(error) = client.set_ip_family(family).await {
            if error.is_unsafe_to_retry() {
                client.release().await;
                return Err(error);
            }
            debug!(family, error = %error, "wds-set-ip-family rejected; continuing");
        }
        match client.start_network(apn, family, profile_id).await {
            Ok(packet_data_handle) => {
                let settings = match client.current_settings().await {
                    Ok(settings) => settings,
                    Err(error) => {
                        // The session is up; without its settings there is no
                        // P-CSCF and no addresses, so it is not usable. Tear it
                        // back down rather than leaking an unusable PDP context.
                        warn!(error = %error, "IMS session started but its settings are unreadable");
                        let _ = client.stop_network(&packet_data_handle).await;
                        client.release().await;
                        return Err(error);
                    }
                };
                return Ok(ImsSession {
                    client,
                    packet_data_handle,
                    ip_family: family,
                    settings,
                });
            }
            Err(error) if error.is_unsafe_to_retry() => {
                client.release().await;
                return Err(error);
            }
            Err(error) => {
                warn!(family, error = %error, "IMS session start failed; trying the next family");
                last_error = Some(error);
            }
        }
    }
    client.release().await;
    Err(last_error.unwrap_or_else(|| WdsError::StartFailed {
        reason: "no ip family was attempted".to_string(),
    }))
}

/// Tear down a session started by [`start_ims_session`] and release its client.
pub async fn stop_ims_session(session: ImsSession) {
    let _ = session
        .client
        .stop_network(&session.packet_data_handle)
        .await;
    session.client.release().await;
}

/// Start a plain data session with a single command.
///
/// This is all a spare rpmsg endpoint can do — no CID survives to read the
/// settings back — which is exactly why the roles are the way round they are:
/// data here, IMS on the primary port.
pub async fn start_single_shot_session(
    endpoint: &WdsEndpoint,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Result<String, WdsError> {
    let mut action = format!("--wds-start-network=apn={apn}");
    if let Some(profile) = profile_id {
        action.push_str(&format!(",3gpp-profile={profile}"));
    }
    action.push_str(&format!(",ip-type={family}"));
    let mut args = endpoint.open_args();
    args.extend_from_slice(&["--client-no-release-cid", action.as_str()]);
    let text = capture(&args, START_TIMEOUT).await?;
    if is_wedge_signature(&text) {
        return Err(WdsError::BasebandWedged(first_error_line(&text)));
    }
    parse_packet_data_handle(&text).ok_or_else(|| WdsError::StartFailed {
        reason: parse_call_end_reason(&text).unwrap_or_else(|| first_error_line(&text)),
    })
}

/// Does this endpoint answer at all, and does the `wds` service exist on it?
///
/// Uses `--get-service-version-info`, which is a CTL query: it neither starts a
/// session nor touches a bearer, so it is safe to run against a port that
/// someone else owns.
pub async fn probe_services(endpoint: &WdsEndpoint) -> Option<String> {
    let mut args = endpoint.open_args();
    args.push("--get-service-version-info");
    capture(&args, QUERY_TIMEOUT).await.ok()
}

/// A usable endpoint must expose the `wds` service — that is what carries the
/// data session.
pub fn advertises_wds(service_listing: &str) -> bool {
    service_listing
        .lines()
        .any(|line| line.trim_start().starts_with("wds"))
}

/// Whether `qmi-proxy` can be reached for this port.
///
/// `--device-open-proxy` starts the proxy on demand, so this is a readiness
/// check rather than a launcher: if it answers, the proxy is up and CID reuse
/// has somewhere to live. Note the binary ships in libexec (`/usr/libexec/qmi-proxy`
/// on the reference device) and is *not* on `PATH`, so `command -v qmi-proxy`
/// finding nothing means nothing.
pub async fn proxy_is_ready(device_path: &str) -> bool {
    let endpoint = WdsEndpoint::primary_via_proxy(device_path);
    probe_services(&endpoint)
        .await
        .is_some_and(|text| advertises_wds(&text))
}

/// IP configuration of a WDS session, as reported by
/// `--wds-get-current-settings`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentSettings {
    pub ip_family: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv4_dns: Vec<String>,
    /// Prefix length for `ipv4_address`, derived from the dotted subnet mask the
    /// modem reports. `ip address add` needs a prefix, not a mask.
    pub ipv4_prefix: Option<u8>,
    pub ipv6_address: Option<String>,
    pub ipv6_gateway: Option<String>,
    pub ipv6_dns: Vec<String>,
    /// Prefix length for `ipv6_address`. qmicli appends it to the address
    /// (`2001:db8::1/64`), so it is split off during parsing.
    pub ipv6_prefix: Option<u8>,
    pub mtu: Option<u32>,
    /// P-CSCF addresses, when the network delivered them via PCO. This is the
    /// whole reason the settings read has to happen on the session's own client.
    pub pcscf: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketServiceStatus {
    Connected,
    Disconnected,
}

/// Parse the two qmicli renderings seen across libqmi versions:
/// `Connection status` and `Packet data connection status`.
pub fn parse_packet_service_status(output: &str) -> Option<PacketServiceStatus> {
    output.lines().find_map(|line| {
        let (label, value) = line.split_once(':')?;
        let label = label.trim().to_ascii_lowercase();
        let label = label
            .rsplit_once(']')
            .map_or(label.as_str(), |(_, suffix)| suffix.trim());
        if label != "connection status" && label != "packet data connection status" {
            return None;
        }
        match value
            .trim()
            .trim_matches('\'')
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "connected" => Some(PacketServiceStatus::Connected),
            "disconnected" => Some(PacketServiceStatus::Disconnected),
            _ => None,
        }
    })
}

impl CurrentSettings {
    /// Nothing usable was reported (e.g. the modem answered `OutOfCall`).
    pub fn is_empty(&self) -> bool {
        self.ipv4_address.is_none() && self.ipv6_address.is_none() && self.pcscf.is_empty()
    }
}

/// Parse the CID out of a `--client-no-release-cid` run.
pub fn parse_retained_cid(output: &str) -> Option<String> {
    let mut seen_retention_notice = false;
    for line in output.lines() {
        if line.contains("Client ID not released") {
            seen_retention_notice = true;
        }
        if !seen_retention_notice {
            continue;
        }
        if let Some((label, value)) = line.rsplit_once(':') {
            if label.trim_end().ends_with("CID") {
                let value = value.trim().trim_matches('\'').trim();
                if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Parse `--wds-start-network` output for the packet data handle.
pub fn parse_packet_data_handle(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("Packet data handle:")?;
        let value = value.trim().trim_matches('\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Extract the modem's verbose call-end reason, e.g. `[3gpp] ipv4-only-allowed`.
/// This is how the network tells us which family it will accept.
pub fn parse_call_end_reason(output: &str) -> Option<String> {
    let verbose = output
        .lines()
        .find(|line| line.contains("verbose call end reason"))
        .map(|line| line.trim().to_string());
    let plain = output
        .lines()
        .find(|line| line.contains("call end reason"))
        .map(|line| line.trim().to_string());
    verbose.or(plain)
}

/// Parse `--wds-get-current-settings` output.
pub fn parse_current_settings(output: &str) -> CurrentSettings {
    let mut settings = CurrentSettings::default();
    for line in output.lines() {
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('\'').trim().to_string();
        if value.is_empty() || value == "none" {
            continue;
        }
        match label.as_str() {
            "ip family" => settings.ip_family = Some(value.to_ascii_lowercase()),
            "ipv4 address" => settings.ipv4_address = Some(value),
            "ipv4 gateway address" => settings.ipv4_gateway = Some(value),
            "ipv4 primary dns" | "ipv4 secondary dns" => settings.ipv4_dns.push(value),
            "ipv4 subnet mask" => settings.ipv4_prefix = prefix_from_ipv4_mask(&value),
            // qmicli renders the IPv6 address with its prefix attached; keep the
            // bare address and record the prefix separately.
            "ipv6 address" => {
                let (address, prefix) = split_prefix(&value);
                settings.ipv6_address = Some(address);
                settings.ipv6_prefix = prefix;
            }
            "ipv6 gateway address" => settings.ipv6_gateway = Some(split_prefix(&value).0),
            "ipv6 primary dns" | "ipv6 secondary dns" => settings.ipv6_dns.push(value),
            "mtu" => settings.mtu = value.parse().ok(),
            "pcscf address" | "p-cscf address" | "pcscf server address" => {
                settings.pcscf.push(value)
            }
            _ => {}
        }
    }
    settings
}

/// Split a `<address>/<prefix>` rendering into its parts. An address without a
/// prefix is returned unchanged with `None`.
fn split_prefix(value: &str) -> (String, Option<u8>) {
    match value.split_once('/') {
        Some((address, prefix)) => (
            address.trim().to_string(),
            prefix.trim().parse::<u8>().ok().filter(|bits| *bits <= 128),
        ),
        None => (value.trim().to_string(), None),
    }
}

/// Convert a dotted IPv4 netmask into a prefix length.
///
/// The reference session reports `255.255.255.224`, i.e. /27. Only contiguous
/// masks are accepted; anything else is reported as unknown so the caller falls
/// back to a host route rather than installing a wrong prefix.
fn prefix_from_ipv4_mask(mask: &str) -> Option<u8> {
    let octets: Vec<u8> = mask
        .split('.')
        .map(str::trim)
        .map(str::parse::<u8>)
        .collect::<Result<_, _>>()
        .ok()?;
    if octets.len() != 4 {
        return None;
    }
    let bits = u32::from_be_bytes([octets[0], octets[1], octets[2], octets[3]]);
    let ones = bits.leading_ones();
    // Reject discontiguous masks: all the set bits must be at the top.
    (bits.count_ones() == ones).then_some(ones as u8)
}

/// First line that looks like an error, for compact diagnostics.
fn first_error_line(text: &str) -> String {
    text.lines()
        .find(|line| line.trim_start().starts_with("error:"))
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| text.trim().replace('\n', " "))
}

async fn capture(args: &[&str], timeout: Duration) -> Result<String, WdsError> {
    let output = run_qmicli(args, timeout).await?;
    Ok(merged_output(&output))
}

fn merged_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(crate) async fn run_qmicli(args: &[&str], timeout: Duration) -> Result<Output, WdsError> {
    match tokio::time::timeout(timeout, Command::new("qmicli").args(args).output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(WdsError::SpawnFailed(error.to_string())),
        Err(_) => Err(WdsError::Timeout(args.join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_is_opened_through_the_proxy_without_net_flags() {
        // Verbatim shape of the command sequence proven on the reference device.
        // The primary port's link format is owned by ModemManager, so the
        // raw-ip flags must not be forced there.
        let primary = WdsEndpoint::primary_via_proxy("/dev/wwan0qmi0");
        assert_eq!(
            primary.open_args(),
            vec!["-d", "/dev/wwan0qmi0", "--device-open-proxy"]
        );
        assert!(primary.supports_ims_flow());
    }

    #[test]
    fn secondary_is_opened_with_the_raw_ip_flags_and_cannot_run_ims() {
        let secondary = WdsEndpoint::secondary("/dev/wwan0qmi1", QmiOpenMode::ForceQmi);
        assert_eq!(
            secondary.open_args(),
            vec![
                "-d",
                "/dev/wwan0qmi1",
                "--device-open-qmi",
                "--device-open-net=net-raw-ip|net-no-qos-header",
            ]
        );
        // Omitting the flags is what made CID allocation fail with "endpoint
        // hangup" on real hardware, so the exact string is load-bearing.
        assert_eq!(
            QMI_OPEN_NET_ARG,
            "--device-open-net=net-raw-ip|net-no-qos-header"
        );
        assert!(!secondary.supports_ims_flow());
    }

    #[tokio::test]
    async fn ims_flow_is_refused_on_an_endpoint_that_cannot_hold_a_cid() {
        // Must fail before spawning anything: a multi-step flow on a bare rpmsg
        // endpoint hangs on step two, and that is a known dead end.
        let secondary = WdsEndpoint::secondary("/dev/wwan0qmi1", QmiOpenMode::ForceQmi);
        let error = WdsClient::allocate(&secondary).await.unwrap_err();
        assert!(matches!(error, WdsError::ImsFlowUnsupported(_)), "{error}");
    }

    #[test]
    fn parses_the_retained_cid_from_real_output() {
        // Verbatim from the reference device.
        let output = "[/dev/wwan0qmi0] Client ID not released:\n\tService: 'wds'\n\t    CID: '2'\n";
        assert_eq!(parse_retained_cid(output).as_deref(), Some("2"));
        // A run that released its client offers no CID to reuse.
        assert!(parse_retained_cid("[/dev/wwan0qmi0] Success\n").is_none());
    }

    #[test]
    fn cid_line_is_not_confused_with_other_labelled_values() {
        let output =
            "[/dev/wwan0qmi0] Client ID not released:\n\tService: 'wds'\n\t    CID: 'abc'\n";
        assert!(parse_retained_cid(output).is_none());
    }

    #[test]
    fn parses_real_start_network_success() {
        // Verbatim from the reference device (Maxis 50212).
        let output = "[/dev/wwan0qmi1] Network started\n\tPacket data handle: '3263198272'\n[/dev/wwan0qmi1] Client ID not released:\n\tService: 'wds'\n\t    CID: '2'\n";
        assert_eq!(
            parse_packet_data_handle(output).as_deref(),
            Some("3263198272")
        );
    }

    #[test]
    fn parses_real_current_settings() {
        // Verbatim from the same successful session.
        let output = "[/dev/wwan0qmi1] Current settings retrieved:\n           IP Family: IPv4\n        IPv4 address: 10.129.39.207\n    IPv4 subnet mask: 255.255.255.224\nIPv4 gateway address: 10.129.39.208\n    IPv4 primary DNS: 172.17.163.218\n  IPv4 secondary DNS: 172.17.167.218\n                 MTU: 1500\n             Domains: none\n";
        let settings = parse_current_settings(output);
        assert_eq!(settings.ip_family.as_deref(), Some("ipv4"));
        assert_eq!(settings.ipv4_address.as_deref(), Some("10.129.39.207"));
        assert_eq!(settings.ipv4_gateway.as_deref(), Some("10.129.39.208"));
        assert_eq!(settings.ipv4_dns, vec!["172.17.163.218", "172.17.167.218"]);
        assert_eq!(settings.mtu, Some(1500));
        // The mask is reported dotted-quad but `ip address add` needs a prefix
        // length, so it is converted at parse time. /27 for 255.255.255.224.
        assert_eq!(settings.ipv4_prefix, Some(27));
        // "Domains: none" must not become a value.
        assert!(settings.ipv6_address.is_none());
        assert!(!settings.is_empty());
    }

    #[test]
    fn subnet_masks_convert_to_prefix_lengths() {
        let prefix_of = |mask: &str| {
            parse_current_settings(&format!(
                "IPv4 address: 10.0.0.2\n    IPv4 subnet mask: {mask}\n"
            ))
            .ipv4_prefix
        };
        assert_eq!(prefix_of("255.255.255.224"), Some(27));
        assert_eq!(prefix_of("255.255.255.255"), Some(32));
        assert_eq!(prefix_of("255.255.255.0"), Some(24));
        assert_eq!(prefix_of("0.0.0.0"), Some(0));
        // A non-contiguous mask is not a valid prefix and must be rejected
        // rather than silently counting bits.
        assert_eq!(prefix_of("255.0.255.0"), None);
        assert_eq!(prefix_of("not-a-mask"), None);
    }

    #[test]
    fn ipv6_address_carries_its_prefix_inline() {
        // qmicli renders the v6 address with the prefix appended, unlike v4.
        let settings = parse_current_settings(
            "           IP Family: IPv6\n        IPv6 address: 2001:db8:1::20/64\n",
        );
        // The address is kept clean for `ip -6 address add`, with the prefix
        // split out into its own field.
        assert_eq!(settings.ipv6_address.as_deref(), Some("2001:db8:1::20"));
        assert_eq!(settings.ipv6_prefix, Some(64));
    }

    #[test]
    fn pcscf_is_picked_up_from_the_settings_read() {
        // The P-CSCF is the reason this step must run on the session's own CID.
        let output = "[/dev/wwan0qmi0] Current settings retrieved:\n           IP Family: IPv4\n        IPv4 address: 10.129.39.207\n       PCSCF address: 10.11.12.13\n       PCSCF address: 10.11.12.14\n";
        let settings = parse_current_settings(output);
        assert_eq!(settings.pcscf, vec!["10.11.12.13", "10.11.12.14"]);
    }

    #[test]
    fn out_of_call_settings_are_reported_as_unusable() {
        // Observed verbatim on the primary port when no session was running.
        // It proves the transaction channel works, but carries no configuration.
        let output = "error: couldn't get current settings: QMI protocol error (15): 'OutOfCall'\n";
        assert!(parse_current_settings(output).is_empty());
        assert_eq!(
            first_error_line(output),
            "error: couldn't get current settings: QMI protocol error (15): 'OutOfCall'"
        );
    }

    #[test]
    fn parses_packet_service_status_across_libqmi_renderings() {
        assert_eq!(
            parse_packet_service_status("[/dev/wwan0qmi0] Connection status: 'connected'\n"),
            Some(PacketServiceStatus::Connected)
        );
        assert_eq!(
            parse_packet_service_status("Packet data connection status: 'disconnected'\n"),
            Some(PacketServiceStatus::Disconnected)
        );
        assert_eq!(
            parse_packet_service_status("Connection status: 'unknown'\n"),
            None
        );
    }

    #[test]
    fn network_forced_family_is_readable_from_call_end_reason() {
        // The reference SIM refuses IPv6 like this; the verbose reason is what
        // tells us to try IPv4, so it must survive parsing.
        let output = "error: couldn't start network: QMI protocol error (14): 'CallFailed'\ncall end reason (1): generic-unspecified\nverbose call end reason (6,50): [3gpp] ipv4-only-allowed\n";
        assert!(parse_packet_data_handle(output).is_none());
        let reason = parse_call_end_reason(output).unwrap();
        assert!(reason.contains("ipv4-only-allowed"), "got: {reason}");
    }

    #[test]
    fn wedge_signatures_are_recognised_and_never_retried() {
        for text in [
            "error: operation failed: endpoint hangup",
            "QMI protocol error (14): CallFailed - interface-in-use-config-match",
            "org.freedesktop.ModemManager1.Error.MobileEquipment.Unknown: internal error",
        ] {
            assert!(is_wedge_signature(text), "{text}");
            assert!(WdsError::BasebandWedged(text.into()).is_unsafe_to_retry());
        }
        // Ordinary family negotiation must stay retryable.
        assert!(!is_wedge_signature("[3gpp] ipv4-only-allowed"));
        assert!(!WdsError::StartFailed {
            reason: "[3gpp] ipv4-only-allowed".into()
        }
        .is_unsafe_to_retry());
    }

    #[test]
    fn wds_detection_matches_real_qmicli_listing() {
        let listing =
            "[/dev/wwan0qmi1] Supported versions:\n\tctl (1.5)\n\twds (1.36)\n\twda (1.11)\n";
        assert!(advertises_wds(listing));
        let no_wds = "[/dev/wwan0qmi3] Supported versions:\n\tctl (1.5)\n\tdms (1.14)\n";
        assert!(!advertises_wds(no_wds));
        assert!(!advertises_wds(""));
    }

    #[test]
    fn bind_arguments_are_never_emitted() {
        // `--wds-bind-data-port` / `--wds-bind-mux-data-port` are unsupported by
        // the target firmware and issuing one restarts the baseband, so nothing
        // in this module may build such an argument.
        //
        // The needle is assembled at runtime and only the code above `mod tests`
        // is scanned, so this check cannot match itself.
        let needle = format!("--wds-{}", "bind");
        let source = include_str!("qmi_wds.rs");
        let code = source
            .split_once("\nmod tests {")
            .map_or(source, |(code, _)| code);
        let emitted: Vec<&str> = code
            .lines()
            .filter(|line| line.contains(&needle))
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
            })
            .collect();
        assert!(
            emitted.is_empty(),
            "bind argument reachable in: {emitted:?}"
        );
    }

    #[test]
    fn error_codes_are_stable_and_prefixed() {
        assert_eq!(
            WdsError::CidReuseUnsupported("x".into()).to_string(),
            "qmi_wds_cid_reuse_unsupported:x"
        );
        assert_eq!(
            WdsError::StartFailed { reason: "r".into() }.to_string(),
            "qmi_wds_start_failed:r"
        );
        assert_eq!(
            WdsError::BasebandWedged("w".into()).to_string(),
            "qmi_wds_baseband_wedged:w"
        );
    }
}
