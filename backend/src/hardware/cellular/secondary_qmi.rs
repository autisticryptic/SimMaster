//! Per-baseband secondary QMI endpoint.
//!
//! # Why this exists
//!
//! ModemManager owns the primary QMI control port (e.g. `/dev/wwan0qmi0`) to run
//! the normal mobile-data bearer. Creating a *second* data session (the IMS/VoLTE
//! bearer) on that same port either fails outright with
//! `interface-in-use-config-match`, or — on the MSM8916-class firmware this
//! project targets — wedges the baseband while activating the IMS PDP context.
//!
//! The fix is physical separation: expose one of the modem's spare control
//! channels as an additional character device and run the IMS bearer there, while
//! ModemManager keeps sole ownership of the primary port.
//!
//! # Portability: discover, don't assume
//!
//! Port naming and metadata differ across platforms and kernels, so nothing here
//! keys off a name:
//!
//!   - Qualcomm SMD/rpmsg (`rpmsg_wwan_ctrl`) publishes channels as `wwan<N>at<M>`
//!     **even when they carry QMI** — the reference device reports
//!     `/sys/class/wwan/wwan0at2/type` = `AT` yet `qmicli --device-open-qmi`
//!     enumerates `wds`, `dms`, `nas`, `uim` on it.
//!   - Other stacks publish `wwan<N>qmi<M>`, `/dev/cdc-wdm<N>` (USB), or MBIM
//!     ports with entirely different names.
//!
//! Therefore an endpoint is accepted only after a **capability probe** actually
//! speaks QMI to it and confirms the `wds` service is present (that is the
//! service the IMS bearer needs). Names and `type` attributes are used purely as
//! ordering hints for which candidate to probe first.
//!
//! # Multi-baseband correctness
//!
//! A host may carry several basebands/readers. Endpoints must never be paired
//! across basebands — line A's IMS session has to run on a channel belonging to
//! line A's modem. Names cannot express that relationship, so pairing is
//! structural: the primary port and each candidate channel are resolved to their
//! owning `<addr>.remoteproc` (or USB parent) sysfs ancestor, and only
//! same-ancestor pairs are considered.
//!
//! Reference topology:
//! ```text
//! /sys/devices/platform/soc@0/4080000.remoteproc/          <- the baseband
//!   ├── wwan/wwan0/wwan0qmi0                                <- primary (ModemManager)
//!   └── remoteproc/remoteproc0/remoteproc0:smd-edge/
//!         └── remoteproc0:smd-edge.DATA6_CNTL.-1.-1         <- secondary source
//! ```
//! `a204000.remoteproc` on the same host is the WCNSS Wi-Fi/BT co-processor, not
//! a baseband; ancestor matching excludes it without special-casing.

use std::{
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};

use tokio::{process::Command, time::sleep};
use tracing::{debug, info, warn};

/// Drivers that can turn a Qualcomm SMD control channel into a WWAN port, in
/// preference order.
///
/// `rpmsg_wwan_ctrl_multi` ships with this project (see
/// `kernel/rpmsg_wwan_ctrl_multi/`) and registers the spare `DATA<n>_CNTL`
/// channels with the correct `WWAN_PORT_QMI` type, so they surface as real QMI
/// ports (`wwan0qmi1`, …).
///
/// The in-tree `rpmsg_wwan_ctrl` is the fallback. It only matches
/// `DATA1`/`DATA4`/`DATA5_CNTL`; a spare channel forced onto it via
/// `driver_override` gets `driver_data == 0` (`WWAN_PORT_UNKNOWN`) and is
/// published as an AT-typed port whose data path is incomplete — CTL queries
/// answer, but WDS client allocation is unreliable and `--wds-start-network` can
/// wedge the baseband. It is still attempted last so a host without the custom
/// module degrades to "probe and reject" rather than silently doing nothing.
const RPMSG_WWAN_DRIVERS: &[&str] = &["rpmsg_wwan_ctrl_multi", "rpmsg_wwan_ctrl"];
const RPMSG_DEVICES_DIR: &str = "/sys/bus/rpmsg/devices";
const RPMSG_DRIVERS_DIR: &str = "/sys/bus/rpmsg/drivers";
const WWAN_CLASS_DIR: &str = "/sys/class/wwan";

/// Ordering hint for which spare SMD channel to try first. Not a whitelist: any
/// `DATA*_CNTL` channel on the same baseband is eligible, these are just tried
/// first because they are the conventional data channels.
const PREFERRED_CHANNELS: &[&str] = &[
    "DATA6_CNTL",
    "DATA7_CNTL",
    "DATA8_CNTL",
    "DATA9_CNTL",
    "DATA5_CNTL",
];

/// Timeout for the kernel to publish a port after `bind`.
const PORT_APPEAR_TIMEOUT: Duration = Duration::from_secs(6);
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Timeout for one capability probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// How a candidate device must be opened by `qmicli`.
///
/// Kernels that mislabel a QMI channel as AT need the mode forced; USB `cdc-wdm`
/// nodes work with the shared proxy. Recorded on the endpoint so every later
/// call reuses the mode that was proven to work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QmiOpenMode {
    /// `--device-open-qmi`: force QMI on a port whose advertised type is wrong.
    ForceQmi,
    /// `--device-open-proxy`: share the port through `qmi-proxy`.
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

/// Data-format flags the secondary endpoint must be opened with.
///
/// This is not cosmetic: on the reference hardware, omitting it makes WDS client
/// allocation fail with `CID allocation failed in the CTL client: endpoint
/// hangup`, while including it allocates reliably. `--wda-get-data-format` on the
/// endpoint confirms why — it reports link-layer `raw-ip` with no QoS header, so
/// the client has to be opened the same way.
pub const QMI_OPEN_NET_ARG: &str = "--device-open-net=net-raw-ip|net-no-qos-header";

/// Result of bringing up an IMS data session on a secondary endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsSession {
    /// WDS packet data handle, needed to stop the session.
    pub packet_data_handle: String,
    /// Family actually established.
    pub ip_family: String,
    pub ipv4_address: Option<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv4_dns: Vec<String>,
    pub ipv6_address: Option<String>,
    pub ipv6_gateway: Option<String>,
    pub ipv6_dns: Vec<String>,
    pub mtu: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondaryQmiEndpoint {
    /// Owning baseband, e.g. `4080000.remoteproc`. Shared with the line's primary
    /// port — this is the multi-baseband pairing key.
    pub remoteproc: String,
    /// Source channel id, e.g. `remoteproc0:smd-edge.DATA6_CNTL.-1.-1`. Empty
    /// when the endpoint was pre-existing and not bound by us.
    pub rpmsg_device: String,
    /// Channel name, e.g. `DATA6_CNTL`.
    pub channel: String,
    /// Port name the kernel published, e.g. `wwan0at2` or `wwan0qmi1`.
    pub port_name: String,
    /// Node to pass to `qmicli -d`, e.g. `/dev/wwan0at2`.
    pub device_path: String,
    /// Open mode proven to work by the capability probe.
    pub open_mode: QmiOpenMode,
    /// rpmsg driver backing this endpoint. Empty for pre-existing endpoints.
    pub driver: String,
    /// Whether this module bound the channel (and so should unbind it).
    pub owned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecondaryQmiError {
    /// Platform lacks the sysfs/driver needed. Callers fall back to the
    /// ModemManager-managed bearer.
    Unsupported(String),
    /// The primary port could not be resolved to an owning baseband.
    PrimaryUnresolved(String),
    /// No usable spare channel on this baseband.
    NoChannelAvailable(String),
    /// Binding failed.
    BindFailed(String),
    /// A port appeared but no open mode could speak QMI/`wds` on it.
    ProbeFailed(String),
}

impl std::fmt::Display for SecondaryQmiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(d) => write!(f, "secondary_qmi_unsupported:{d}"),
            Self::PrimaryUnresolved(d) => write!(f, "secondary_qmi_primary_unresolved:{d}"),
            Self::NoChannelAvailable(d) => write!(f, "secondary_qmi_no_channel:{d}"),
            Self::BindFailed(d) => write!(f, "secondary_qmi_bind_failed:{d}"),
            Self::ProbeFailed(d) => write!(f, "secondary_qmi_probe_failed:{d}"),
        }
    }
}

impl std::error::Error for SecondaryQmiError {}

/// Enumerate the primary QMI control ports present on this host, one per
/// baseband, straight from sysfs.
///
/// Used by `secondary-qmi-init`, which runs *before* ModemManager and therefore
/// cannot ask it for the modem inventory. Ports whose advertised type is QMI are
/// returned, deduplicated by owning baseband so a host with several modems yields
/// one primary per modem.
pub fn discover_primary_qmi_ports() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(WWAN_CLASS_DIR) else {
        return Vec::new();
    };
    let mut by_baseband: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(resolved) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        // Only real QMI ports are primaries.
        let port_type = std::fs::read_to_string(resolved.join("type"))
            .ok()
            .map(|t| t.trim().to_string());
        if !port_type.is_some_and(|t| t.eq_ignore_ascii_case("QMI")) {
            continue;
        }
        if !Path::new(&format!("/dev/{name}")).exists() {
            continue;
        }
        let Some(baseband) = remoteproc_of_path(&resolved.to_string_lossy()) else {
            continue;
        };
        // Lowest-numbered QMI port on a baseband is the one ModemManager takes.
        by_baseband
            .entry(baseband)
            .and_modify(|current| {
                if name < *current {
                    *current = name.clone();
                }
            })
            .or_insert_with(|| name.clone());
    }
    by_baseband
        .into_values()
        .map(|port| format!("/dev/{port}"))
        .collect()
}

/// Extract the owning `<addr>.remoteproc` component from a resolved sysfs path.
/// Pure helper so the pairing rule is testable without sysfs.
pub fn remoteproc_of_path(resolved: &str) -> Option<String> {
    resolved
        .split('/')
        .find(|component| component.ends_with(".remoteproc"))
        .map(str::to_string)
}

/// Resolve a device node to the sysfs ancestor identifying its baseband.
///
/// Tries the WWAN class first (platform/rpmsg modems), then falls back to a USB
/// parent for `cdc-wdm`-style nodes, so the pairing key works on both stacks.
pub fn baseband_key_for_device(device_path: &str) -> Result<String, SecondaryQmiError> {
    let port = device_path.rsplit('/').next().unwrap_or(device_path);

    let class_link = PathBuf::from(WWAN_CLASS_DIR).join(port);
    if class_link.exists() {
        let resolved = std::fs::canonicalize(&class_link).map_err(|error| {
            SecondaryQmiError::PrimaryUnresolved(format!("{}: {error}", class_link.display()))
        })?;
        let resolved = resolved.to_string_lossy().to_string();
        if let Some(remoteproc) = remoteproc_of_path(&resolved) {
            return Ok(remoteproc);
        }
        // Non-remoteproc WWAN (e.g. USB): use the port's parent device directory,
        // which is stable per physical modem.
        if let Some(parent) = Path::new(&resolved).parent() {
            return Ok(parent.to_string_lossy().to_string());
        }
    }

    // USB character devices are not in the wwan class; resolve via /sys/class/usbmisc.
    let usbmisc = PathBuf::from("/sys/class/usbmisc").join(port);
    if usbmisc.exists() {
        if let Ok(resolved) = std::fs::canonicalize(&usbmisc) {
            // Walk up to the USB interface's parent device (the modem).
            let resolved = resolved.to_string_lossy().to_string();
            if let Some(index) = resolved.find("/usbmisc/") {
                return Ok(resolved[..index].to_string());
            }
            return Ok(resolved);
        }
    }

    Err(SecondaryQmiError::PrimaryUnresolved(format!(
        "cannot resolve a baseband for {device_path}"
    )))
}

/// Backwards-compatible alias used by the VoLTE code path.
pub fn remoteproc_for_primary(device_path: &str) -> Result<String, SecondaryQmiError> {
    baseband_key_for_device(device_path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RpmsgChannel {
    device_id: String,
    channel: String,
    baseband: String,
    bound_driver: Option<String>,
}

/// Enumerate spare `DATA*_CNTL` rpmsg channels, resolved to their baseband.
fn enumerate_channels() -> Result<Vec<RpmsgChannel>, SecondaryQmiError> {
    let dir = Path::new(RPMSG_DEVICES_DIR);
    if !dir.exists() {
        return Err(SecondaryQmiError::Unsupported(format!(
            "{RPMSG_DEVICES_DIR} is absent"
        )));
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| SecondaryQmiError::Unsupported(format!("{RPMSG_DEVICES_DIR}: {e}")))?;

    let mut channels = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(channel) = std::fs::read_to_string(path.join("name")) else {
            continue;
        };
        let channel = channel.trim().to_string();
        if !is_spare_data_channel(&channel) {
            continue;
        }
        if !path.join("driver_override").exists() {
            continue;
        }
        let Ok(resolved) = std::fs::canonicalize(&path) else {
            continue;
        };
        let Some(baseband) = remoteproc_of_path(&resolved.to_string_lossy()) else {
            continue;
        };
        let bound_driver = std::fs::canonicalize(path.join("driver"))
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
            .filter(|name| name != "driver");
        channels.push(RpmsgChannel {
            device_id: entry.file_name().to_string_lossy().to_string(),
            channel,
            baseband,
            bound_driver,
        });
    }
    Ok(channels)
}

/// Any `DATA<n>_CNTL` channel qualifies; the preferred list only orders probing.
pub fn is_spare_data_channel(channel: &str) -> bool {
    channel.starts_with("DATA") && channel.ends_with("_CNTL")
}

/// Rank a channel for probe order: lower is tried first.
fn channel_rank(channel: &str) -> usize {
    PREFERRED_CHANNELS
        .iter()
        .position(|preferred| *preferred == channel)
        .unwrap_or(PREFERRED_CHANNELS.len())
}

/// Ports currently published for a baseband, with their advertised type.
fn ports_for_baseband(baseband: &str) -> Vec<(String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(WWAN_CLASS_DIR) else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(resolved) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        if remoteproc_of_path(&resolved.to_string_lossy()).as_deref() != Some(baseband) {
            continue;
        }
        let port_type = std::fs::read_to_string(resolved.join("type"))
            .ok()
            .map(|t| t.trim().to_string());
        ports.push((name, port_type));
    }
    ports.sort_by(|a, b| a.0.cmp(&b.0));
    ports
}

/// Every non-primary QMI port on this host, one entry per port.
///
/// ModemManager must be kept off *all* of them, not just the one that ends up
/// carrying a session: the custom module publishes a spare port per registered
/// channel (DATA6 *and* DATA7), and any spare left visible is enumerated by
/// ModemManager as an extra modem port. On the reference device that left
/// `wwan0qmi2` claimable while `wwan0qmi1` was correctly ignored.
///
/// The primary of each baseband is excluded — that one is ModemManager's, by
/// design. Boot AT consoles are excluded because they are not QMI at all.
pub fn discover_spare_qmi_ports() -> Vec<String> {
    let primaries: Vec<String> = discover_primary_qmi_ports()
        .into_iter()
        .map(|device| device.rsplit('/').next().unwrap_or_default().to_string())
        .collect();
    let Ok(entries) = std::fs::read_dir(WWAN_CLASS_DIR) else {
        return Vec::new();
    };
    let mut spares = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if primaries.contains(&name) || is_boot_port(&name) {
            continue;
        }
        let Ok(resolved) = std::fs::canonicalize(entry.path()) else {
            continue;
        };
        // Only QMI-typed ports; an AT port cannot host a WDS session and
        // ModemManager may legitimately want it.
        let is_qmi = std::fs::read_to_string(resolved.join("type"))
            .ok()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("QMI"));
        if is_qmi && remoteproc_of_path(&resolved.to_string_lossy()).is_some() {
            spares.push(name);
        }
    }
    spares.sort();
    spares
}

/// The udev line that hides one port from ModemManager.
pub fn udev_ignore_rule(port_name: &str) -> String {
    format!("SUBSYSTEM==\"wwan\", KERNEL==\"{port_name}\", ENV{{ID_MM_PORT_IGNORE}}=\"1\"")
}

/// Probe-order hint for a discovered port: prefer names that advertise QMI, then
/// anything else. Naming is only a hint — the capability probe decides.
fn port_rank(port: &str, port_type: Option<&str>) -> usize {
    let name_says_qmi = port.contains("qmi");
    let type_says_qmi = port_type.is_some_and(|t| t.eq_ignore_ascii_case("QMI"));
    match (type_says_qmi, name_says_qmi) {
        (true, _) => 0,
        (_, true) => 1,
        _ => 2,
    }
}

fn driver_bind_path(driver: &str) -> PathBuf {
    PathBuf::from(RPMSG_DRIVERS_DIR).join(driver).join("bind")
}

fn driver_unbind_path(driver: &str) -> PathBuf {
    PathBuf::from(RPMSG_DRIVERS_DIR).join(driver).join("unbind")
}

/// Drivers present on this host, in preference order. The custom multi driver is
/// preferred because it types spare channels as real QMI ports.
fn available_drivers() -> Vec<&'static str> {
    RPMSG_WWAN_DRIVERS
        .iter()
        .copied()
        .filter(|driver| driver_bind_path(driver).exists())
        .collect()
}

/// Is this driver one of ours (i.e. safe to rebind a channel away from)?
fn is_wwan_ctrl_driver(driver: &str) -> bool {
    RPMSG_WWAN_DRIVERS.contains(&driver)
}

/// Does this device actually speak QMI with the `wds` service, and in which mode?
///
/// This is the portability keystone: it replaces every assumption about port
/// names and `type` attributes with an observation.
pub async fn probe_qmi_capability(device_path: &str) -> Option<QmiOpenMode> {
    for mode in QmiOpenMode::probe_order() {
        let output = run_qmicli(&[
            "-d",
            device_path,
            mode.as_arg(),
            QMI_OPEN_NET_ARG,
            "--get-service-version-info",
        ])
        .await;
        let Some(output) = output else { continue };
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if advertises_wds(&text) {
            debug!(device = %device_path, mode = ?mode, "QMI capability probe succeeded");
            return Some(mode);
        }
    }
    None
}

/// A usable endpoint must expose the `wds` service — that is what carries the
/// IMS data session.
pub fn advertises_wds(service_listing: &str) -> bool {
    service_listing
        .lines()
        .any(|line| line.trim_start().starts_with("wds"))
}

async fn run_qmicli(args: &[&str]) -> Option<Output> {
    match tokio::time::timeout(PROBE_TIMEOUT, Command::new("qmicli").args(args).output()).await {
        Ok(Ok(output)) => Some(output),
        Ok(Err(error)) => {
            debug!(error = %error, "qmicli spawn failed");
            None
        }
        Err(_) => {
            debug!(?args, "qmicli probe timed out");
            None
        }
    }
}

/// Ensure a secondary QMI endpoint exists for the baseband owning
/// `primary_device`, and return it.
///
/// Order of preference:
///   1. an already-published extra port on this baseband that passes the probe
///      (covers kernels/platforms that expose a second QMI port natively, and
///      re-attach after a restart),
///   2. otherwise bind a spare `DATA*_CNTL` channel and probe the port it creates.
pub async fn ensure_endpoint(
    primary_device: &str,
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    let baseband = baseband_key_for_device(primary_device)?;
    let primary_port = primary_device
        .rsplit('/')
        .next()
        .unwrap_or(primary_device)
        .to_string();

    // 1. Reuse any extra port already present on this baseband.
    let mut existing: Vec<(String, Option<String>)> = ports_for_baseband(&baseband)
        .into_iter()
        .filter(|(name, _)| *name != primary_port && !is_boot_port(name))
        .collect();
    existing.sort_by_key(|(name, port_type)| port_rank(name, port_type.as_deref()));
    for (port, _) in existing {
        let device_path = format!("/dev/{port}");
        if !Path::new(&device_path).exists() {
            continue;
        }
        if let Some(open_mode) = probe_qmi_capability(&device_path).await {
            info!(
                baseband = %baseband,
                port = %port,
                mode = ?open_mode,
                "Reusing existing secondary QMI endpoint"
            );
            return Ok(SecondaryQmiEndpoint {
                remoteproc: baseband,
                rpmsg_device: String::new(),
                channel: String::from("preexisting"),
                port_name: port,
                device_path,
                open_mode,
                driver: String::new(),
                owned: false,
            });
        }
    }

    // 2. Bind a spare channel on this baseband.
    let drivers = available_drivers();
    if drivers.is_empty() {
        return Err(SecondaryQmiError::Unsupported(format!(
            "none of {RPMSG_WWAN_DRIVERS:?} is loaded and no probe-capable port exists"
        )));
    }
    let channels = enumerate_channels()?;
    let mut mine: Vec<&RpmsgChannel> = channels
        .iter()
        .filter(|channel| channel.baseband == baseband)
        .filter(|channel| {
            // Free, or already held by one of our drivers (safe to rebind).
            channel
                .bound_driver
                .as_deref()
                .is_none_or(is_wwan_ctrl_driver)
        })
        .collect();
    if mine.is_empty() {
        return Err(SecondaryQmiError::NoChannelAvailable(format!(
            "no free DATA*_CNTL channel under {baseband}"
        )));
    }
    mine.sort_by_key(|channel| channel_rank(&channel.channel));

    let before: Vec<String> = ports_for_baseband(&baseband)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut last_error = None;
    // Prefer the driver that types spare channels correctly; only fall back to
    // the in-tree driver (which mistypes them) if the custom one is absent.
    for driver in &drivers {
        for candidate in &mine {
            match bind_and_probe(candidate, driver, &baseband, &before).await {
                Ok(endpoint) => return Ok(endpoint),
                Err(error) => {
                    warn!(
                        channel = %candidate.channel,
                        driver = %driver,
                        error = %error,
                        "Secondary QMI attempt failed; trying next candidate"
                    );
                    last_error = Some(error);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        SecondaryQmiError::NoChannelAvailable(format!(
            "no DATA*_CNTL channel under {baseband} yielded a QMI-capable port"
        ))
    }))
}

/// Boot-time ports that are never the secondary endpoint (the modem's own AT
/// consoles). Everything else is a candidate and gets probed.
fn is_boot_port(port: &str) -> bool {
    port.ends_with("at0") || port.ends_with("at1")
}

async fn bind_and_probe(
    candidate: &RpmsgChannel,
    driver: &str,
    baseband: &str,
    ports_before: &[String],
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    // If the channel is currently held by the other wwan-ctrl driver, detach it
    // first so driver_override can steer it to the one we want.
    if candidate
        .bound_driver
        .as_deref()
        .is_some_and(|bound| bound != driver)
    {
        if let Some(bound) = candidate.bound_driver.as_deref() {
            let _ = std::fs::write(driver_unbind_path(bound), &candidate.device_id);
            sleep(PORT_POLL_INTERVAL).await;
        }
    }
    let device_dir = PathBuf::from(RPMSG_DEVICES_DIR).join(&candidate.device_id);
    std::fs::write(device_dir.join("driver_override"), driver).map_err(|e| {
        SecondaryQmiError::BindFailed(format!("driver_override {}: {e}", candidate.channel))
    })?;
    if let Err(error) = std::fs::write(driver_bind_path(driver), &candidate.device_id) {
        // EBUSY means it is already bound — that is fine, the port may exist.
        debug!(channel = %candidate.channel, error = %error, "bind write returned an error");
    }

    let deadline = tokio::time::Instant::now() + PORT_APPEAR_TIMEOUT;
    let mut discovered: Option<(String, Option<String>)> = None;
    while tokio::time::Instant::now() < deadline {
        let mut fresh: Vec<(String, Option<String>)> = ports_for_baseband(baseband)
            .into_iter()
            .filter(|(name, _)| !ports_before.contains(name))
            .collect();
        if !fresh.is_empty() {
            fresh.sort_by_key(|(name, port_type)| port_rank(name, port_type.as_deref()));
            discovered = fresh.into_iter().next();
            break;
        }
        sleep(PORT_POLL_INTERVAL).await;
    }

    let Some((port, port_type)) = discovered else {
        release_endpoint_by_device(&candidate.device_id, driver).await;
        return Err(SecondaryQmiError::BindFailed(format!(
            "{} bound via {driver} but no new port appeared under {baseband}",
            candidate.channel
        )));
    };

    let device_path = format!("/dev/{port}");
    // Give udev a moment to create the node.
    for _ in 0..12 {
        if Path::new(&device_path).exists() {
            break;
        }
        sleep(PORT_POLL_INTERVAL).await;
    }

    match probe_qmi_capability(&device_path).await {
        Some(open_mode) => {
            info!(
                baseband = %baseband,
                channel = %candidate.channel,
                driver = %driver,
                port = %port,
                advertised_type = ?port_type,
                mode = ?open_mode,
                "Secondary QMI endpoint ready"
            );
            Ok(SecondaryQmiEndpoint {
                remoteproc: baseband.to_string(),
                rpmsg_device: candidate.device_id.clone(),
                channel: candidate.channel.clone(),
                port_name: port,
                device_path,
                open_mode,
                driver: driver.to_string(),
                owned: true,
            })
        }
        None => {
            release_endpoint_by_device(&candidate.device_id, driver).await;
            Err(SecondaryQmiError::ProbeFailed(format!(
                "{port} (type {port_type:?}, driver {driver}) does not expose the wds service"
            )))
        }
    }
}

/// Unbind a channel from `driver` and clear its driver override.
pub async fn release_endpoint_by_device(rpmsg_device: &str, driver: &str) {
    if rpmsg_device.is_empty() {
        return;
    }
    if let Err(error) = std::fs::write(driver_unbind_path(driver), rpmsg_device) {
        debug!(device = %rpmsg_device, driver = %driver, error = %error, "Secondary QMI unbind failed");
    }
    let override_path = PathBuf::from(RPMSG_DEVICES_DIR)
        .join(rpmsg_device)
        .join("driver_override");
    if let Err(error) = std::fs::write(&override_path, "\n") {
        debug!(device = %rpmsg_device, error = %error, "Clearing driver_override failed");
    }
}

/// Release an endpoint this module bound. Pre-existing endpoints are left alone.
pub async fn release_endpoint(endpoint: &SecondaryQmiEndpoint) {
    if endpoint.owned && !endpoint.driver.is_empty() {
        release_endpoint_by_device(&endpoint.rpmsg_device, &endpoint.driver).await;
    }
}

/// Start the IMS data session on this endpoint for one address family.
///
/// `family` is the QMI `ip-type` value: `4` or `6`. On success the session's IP
/// configuration is read back with `--wds-get-current-settings`, which is also
/// where the P-CSCF (delivered via PCO) shows up.
pub async fn start_ims_session(
    endpoint: &SecondaryQmiEndpoint,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Result<ImsSession, String> {
    let mut start = format!("--wds-start-network=apn={apn}");
    if let Some(profile) = profile_id {
        start.push_str(&format!(",3gpp-profile={profile}"));
    }
    start.push_str(&format!(",ip-type={family}"));

    let output = run_qmicli(&[
        "-d",
        &endpoint.device_path,
        endpoint.open_mode.as_arg(),
        QMI_OPEN_NET_ARG,
        "--client-no-release-cid",
        &start,
    ])
    .await
    .ok_or_else(|| "secondary_qmi_start_spawn_failed".to_string())?;

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let handle = parse_packet_data_handle(&text).ok_or_else(|| {
        // Surface the modem's own verbose reason: it distinguishes "network wants
        // the other family" from a real failure.
        let reason = parse_call_end_reason(&text).unwrap_or_else(|| text.trim().to_string());
        format!("secondary_qmi_start_failed:{reason}")
    })?;

    let settings = read_current_settings(endpoint).await.unwrap_or_default();
    Ok(ImsSession {
        packet_data_handle: handle,
        ip_family: settings.ip_family.unwrap_or_else(|| format!("ipv{family}")),
        ipv4_address: settings.ipv4_address,
        ipv4_gateway: settings.ipv4_gateway,
        ipv4_dns: settings.ipv4_dns,
        ipv6_address: settings.ipv6_address,
        ipv6_gateway: settings.ipv6_gateway,
        ipv6_dns: settings.ipv6_dns,
        mtu: settings.mtu,
    })
}

/// Tear down an IMS session started by [`start_ims_session`].
pub async fn stop_ims_session(endpoint: &SecondaryQmiEndpoint, handle: &str) {
    let stop = format!("--wds-stop-network={handle}");
    let _ = run_qmicli(&[
        "-d",
        &endpoint.device_path,
        endpoint.open_mode.as_arg(),
        QMI_OPEN_NET_ARG,
        &stop,
    ])
    .await;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentSettings {
    pub ip_family: Option<String>,
    pub ipv4_address: Option<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv4_dns: Vec<String>,
    pub ipv6_address: Option<String>,
    pub ipv6_gateway: Option<String>,
    pub ipv6_dns: Vec<String>,
    pub mtu: Option<u32>,
    /// P-CSCF addresses if the network delivered them via PCO.
    pub pcscf: Vec<String>,
}

async fn read_current_settings(endpoint: &SecondaryQmiEndpoint) -> Option<CurrentSettings> {
    let output = run_qmicli(&[
        "-d",
        &endpoint.device_path,
        endpoint.open_mode.as_arg(),
        QMI_OPEN_NET_ARG,
        "--wds-get-current-settings",
    ])
    .await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(parse_current_settings(&text))
}

/// Parse `qmicli --wds-start-network` output for the packet data handle.
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

/// Parse `qmicli --wds-get-current-settings` output.
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
            "ipv6 address" => settings.ipv6_address = Some(value),
            "ipv6 gateway address" => settings.ipv6_gateway = Some(value),
            "ipv6 primary dns" | "ipv6 secondary dns" => settings.ipv6_dns.push(value),
            "mtu" => settings.mtu = value.parse().ok(),
            "pcscf address" | "p-cscf address" => settings.pcscf.push(value),
            _ => {}
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remoteproc_extracted_from_sysfs_path() {
        let primary = "/sys/devices/platform/soc@0/4080000.remoteproc/wwan/wwan0/wwan0qmi0";
        assert_eq!(
            remoteproc_of_path(primary).as_deref(),
            Some("4080000.remoteproc")
        );
        let rpmsg = "/sys/devices/platform/soc@0/4080000.remoteproc/remoteproc/remoteproc0/remoteproc0:smd-edge/remoteproc0:smd-edge.DATA6_CNTL.-1.-1";
        assert_eq!(
            remoteproc_of_path(rpmsg).as_deref(),
            Some("4080000.remoteproc")
        );
    }

    #[test]
    fn different_coprocessors_never_pair() {
        let baseband = "/sys/devices/platform/soc@0/4080000.remoteproc/wwan/wwan0/wwan0qmi0";
        let wcnss = "/sys/devices/platform/soc@0/a204000.remoteproc/remoteproc/remoteproc1/remoteproc1:smd-edge/remoteproc1:smd-edge.WCNSS_CTRL.-1.-1";
        assert_ne!(remoteproc_of_path(baseband), remoteproc_of_path(wcnss));
    }

    #[test]
    fn each_baseband_resolves_to_its_own_key() {
        let a = "/sys/devices/platform/soc@0/4080000.remoteproc/wwan/wwan0/wwan0qmi0";
        let b = "/sys/devices/platform/soc@0/6080000.remoteproc/wwan/wwan1/wwan1qmi0";
        assert_eq!(remoteproc_of_path(a).as_deref(), Some("4080000.remoteproc"));
        assert_eq!(remoteproc_of_path(b).as_deref(), Some("6080000.remoteproc"));
    }

    #[test]
    fn any_data_cntl_channel_is_eligible() {
        // Not restricted to DATA6 — portability across firmware channel layouts.
        assert!(is_spare_data_channel("DATA6_CNTL"));
        assert!(is_spare_data_channel("DATA5_CNTL"));
        assert!(is_spare_data_channel("DATA40_CNTL"));
        assert!(!is_spare_data_channel("DIAG_CNTL"));
        assert!(!is_spare_data_channel("DATA1"));
    }

    #[test]
    fn preferred_channels_are_probed_first_but_others_still_rank() {
        assert_eq!(channel_rank("DATA6_CNTL"), 0);
        assert!(channel_rank("DATA7_CNTL") < channel_rank("DATA40_CNTL"));
        // Unlisted channels are still eligible, just last.
        assert_eq!(channel_rank("DATA40_CNTL"), PREFERRED_CHANNELS.len());
    }

    #[test]
    fn port_ranking_prefers_declared_qmi_then_qmi_names_then_anything() {
        // A port whose type says QMI wins.
        assert_eq!(port_rank("wwan0at2", Some("QMI")), 0);
        // Then a qmi-looking name.
        assert_eq!(port_rank("wwan0qmi1", Some("AT")), 1);
        // The reference device: AT-named, AT-typed, yet carries QMI — still a
        // candidate, just probed last.
        assert_eq!(port_rank("wwan0at2", Some("AT")), 2);
        assert!(port_rank("wwan0qmi1", None) < port_rank("wwan0at2", None));
    }

    #[test]
    fn boot_at_consoles_are_not_candidates() {
        assert!(is_boot_port("wwan0at0"));
        assert!(is_boot_port("wwan0at1"));
        assert!(!is_boot_port("wwan0at2"));
        assert!(!is_boot_port("wwan0qmi1"));
        // Multi-baseband naming is handled by the suffix rule too.
        assert!(is_boot_port("wwan1at0"));
        assert!(!is_boot_port("wwan1at2"));
    }

    #[test]
    fn wds_detection_matches_real_qmicli_listing() {
        // Trimmed from the reference device's actual output.
        let listing = "[/dev/wwan0at2] Supported versions:\n\tctl (1.5)\n\twds (1.36)\n\tdms (1.14)\n\tuim (1.36)\n";
        assert!(advertises_wds(listing));
        // A port without wds must be rejected even if it answers QMI.
        let no_wds = "[/dev/wwan0at3] Supported versions:\n\tctl (1.5)\n\tdms (1.14)\n";
        assert!(!advertises_wds(no_wds));
        assert!(!advertises_wds(""));
    }

    #[test]
    fn open_mode_args_and_probe_order() {
        assert_eq!(QmiOpenMode::ForceQmi.as_arg(), "--device-open-qmi");
        assert_eq!(QmiOpenMode::Proxy.as_arg(), "--device-open-proxy");
        assert_eq!(
            QmiOpenMode::probe_order(),
            [QmiOpenMode::ForceQmi, QmiOpenMode::Proxy]
        );
    }

    #[test]
    fn custom_multi_driver_is_preferred_over_intree() {
        // The in-tree driver mistypes spare channels as AT, so the module that
        // types them as QMI must be tried first.
        assert_eq!(RPMSG_WWAN_DRIVERS[0], "rpmsg_wwan_ctrl_multi");
        assert_eq!(RPMSG_WWAN_DRIVERS[1], "rpmsg_wwan_ctrl");
        assert!(is_wwan_ctrl_driver("rpmsg_wwan_ctrl_multi"));
        assert!(is_wwan_ctrl_driver("rpmsg_wwan_ctrl"));
        // A channel held by an unrelated driver must not be stolen.
        assert!(!is_wwan_ctrl_driver("rpmsg_chrdev"));
        assert!(!is_wwan_ctrl_driver("qcom_smd_qrtr"));
    }

    #[test]
    fn driver_paths_are_per_driver() {
        // Compare components so the assertion holds regardless of the host's
        // path separator (tests also run on Windows dev machines).
        let bind = driver_bind_path("rpmsg_wwan_ctrl_multi");
        let mut bind_tail = bind.components().rev().take(2).collect::<Vec<_>>();
        bind_tail.reverse();
        assert_eq!(
            bind_tail
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["rpmsg_wwan_ctrl_multi", "bind"]
        );

        let unbind = driver_unbind_path("rpmsg_wwan_ctrl");
        let mut unbind_tail = unbind.components().rev().take(2).collect::<Vec<_>>();
        unbind_tail.reverse();
        assert_eq!(
            unbind_tail
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            vec!["rpmsg_wwan_ctrl", "unbind"]
        );
    }

    #[test]
    fn parses_real_start_network_success() {
        // Verbatim from the reference device (192.168.100.13, Maxis) after the
        // custom module exposed /dev/wwan0qmi1 as a real QMI port.
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
        // "Domains: none" must not become a value.
        assert!(settings.ipv6_address.is_none());
    }

    #[test]
    fn network_forced_family_is_readable_from_call_end_reason() {
        // The network refusing the other family looks like this — the verbose
        // reason is what tells us to switch, so it must survive parsing.
        let output = "error: couldn't start network: QMI protocol error (14): 'CallFailed'\ncall end reason (1): generic-unspecified\nverbose call end reason (6,50): [3gpp] ipv4-only-allowed\n";
        assert!(parse_packet_data_handle(output).is_none());
        let reason = parse_call_end_reason(output).unwrap();
        assert!(reason.contains("ipv4-only-allowed"), "got: {reason}");
    }

    #[test]
    fn raw_ip_open_arg_is_present_and_exact() {
        // Omitting this makes WDS CID allocation fail with "endpoint hangup" on
        // real hardware, so the exact flag string is load-bearing.
        assert_eq!(
            QMI_OPEN_NET_ARG,
            "--device-open-net=net-raw-ip|net-no-qos-header"
        );
    }

    #[test]
    fn error_codes_are_stable_and_prefixed() {
        assert_eq!(
            SecondaryQmiError::Unsupported("x".into()).to_string(),
            "secondary_qmi_unsupported:x"
        );
        assert_eq!(
            SecondaryQmiError::ProbeFailed("z".into()).to_string(),
            "secondary_qmi_probe_failed:z"
        );
    }
}
