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
use tracing::{debug, info};

/// Beta8 deliberately migrated DATA6 away from the kernel-specific multi-port
/// module and binds exactly one channel through the in-tree driver. On MSM8916,
/// trying DATA7/8/9 after DATA6 fails can tear down the primary modem inventory,
/// so this list must remain singular.
const RPMSG_WWAN_DRIVER: &str = "rpmsg_wwan_ctrl";
const LEGACY_RPMSG_WWAN_DRIVER: &str = "rpmsg_wwan_ctrl_multi";
const SECONDARY_CHANNEL: &str = "DATA6_CNTL";
const RPMSG_DEVICES_DIR: &str = "/sys/bus/rpmsg/devices";
const RPMSG_DRIVERS_DIR: &str = "/sys/bus/rpmsg/drivers";
const WWAN_CLASS_DIR: &str = "/sys/class/wwan";
const NET_CLASS_DIR: &str = "/sys/class/net";
pub const SECONDARY_QMI_STATE_FILE: &str = "/run/simadmin/secondary-qmi-device";
pub const SECONDARY_QMI_ENDPOINTS_STATE_FILE: &str = "/run/simadmin/secondary-qmi-endpoints.json";

/// DATA6 is an optional hardware capability, not a safe default on every
/// MSM8916 firmware.  In particular, the 410 firmware used by SimAdmin can
/// crash the modem when the AT-labelled DATA6 channel is force-opened as QMI.
/// Keep the primary ModemManager QMI path usable unless an operator explicitly
/// opts in after validating the device firmware.
pub fn secondary_qmi_enabled() -> bool {
    std::env::var("SIMADMIN_ENABLE_SECONDARY_QMI")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Timeout for the kernel to publish a port after `bind`.
const PORT_APPEAR_TIMEOUT: Duration = Duration::from_secs(6);
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long the primary-port set must stay unchanged before it counts as final.
/// Long enough for a second baseband attaching a little later to be picked up,
/// short enough not to delay ModemManager on a single-modem host.
const PRIMARY_PORT_SETTLE: Duration = Duration::from_secs(2);
/// Upper bound on waiting for the first primary QMI port to appear. Hosts with
/// no QMI hardware at all pay this once per boot and then fall through.
pub const PRIMARY_PORT_WAIT: Duration = Duration::from_secs(45);

/// Where an older install may have left the out-of-tree multi-port module.
const LEGACY_MODULE_SYSFS: &str = "/sys/module/rpmsg_wwan_ctrl_multi";
const LEGACY_MODULE_FILES: &[&str] = &[
    "/opt/simadmin/modules/rpmsg_wwan_ctrl_multi.ko",
    "/lib/modules/{kver}/extra/simadmin/rpmsg_wwan_ctrl_multi.ko",
    "/lib/modules/{kver}/extra/rpmsg_wwan_ctrl_multi.ko",
];

/// Unload and delete the legacy out-of-tree multi-port RPMSG module.
///
/// `rpmsg_wwan_ctrl_multi` published one WWAN port per channel in its id_table.
/// DATA6 now binds through the in-tree `rpmsg_wwan_ctrl` instead, so the module
/// is redundant — but leaving it installed is not merely untidy. While it stays
/// loaded it keeps auto-binding *every* matching `DATA*_CNTL` channel on each
/// boot, and those extra binds land on the modem while it is still bringing up
/// Data Services Memory. On this MSM8916 firmware that is what takes the DSP
/// down with `smd_dsm_memcpy.c` a second after mpss starts, which in turn
/// latches `bam-dmux` runtime PM at `error` and leaves every `wwanN` unusable
/// for the rest of the boot.
///
/// Deliberately runs whether or not DATA6 is enabled: with DATA6 off the module
/// has no purpose whatsoever, so keeping it loaded is pure risk. Every step is
/// best-effort — a read-only rootfs or a module built into the kernel must not
/// stop the rest of initialization.
pub async fn purge_legacy_rpmsg_module() -> bool {
    let mut changed = false;

    if Path::new(LEGACY_MODULE_SYSFS).exists() {
        match Command::new("rmmod")
            .arg(LEGACY_RPMSG_WWAN_DRIVER)
            .status()
            .await
        {
            Ok(status) if status.success() => {
                info!(
                    module = LEGACY_RPMSG_WWAN_DRIVER,
                    "Unloaded the legacy multi-port RPMSG module"
                );
                changed = true;
            }
            Ok(status) => debug!(
                module = LEGACY_RPMSG_WWAN_DRIVER,
                ?status,
                "rmmod refused; the module may still be in use"
            ),
            Err(error) => debug!(error = %error, "rmmod is unavailable"),
        }
    }

    let kver = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    for template in LEGACY_MODULE_FILES {
        let path = template.replace("{kver}", &kver);
        if Path::new(&path).exists() && std::fs::remove_file(&path).is_ok() {
            info!(path = %path, "Removed the legacy multi-port RPMSG module file");
            changed = true;
        }
    }

    if changed && !kver.is_empty() {
        let _ = Command::new("depmod").arg("-a").arg(&kver).status().await;
    }
    changed
}

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
}

impl QmiOpenMode {
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::ForceQmi => "--device-open-qmi",
        }
    }

    /// Beta8's boot probe is direct and forces QMI because the stock driver
    /// advertises DATA6 as an UNKNOWN/AT-style port. Proxy sharing starts only
    /// for the retained runtime WDS client.
    pub fn probe_order() -> [Self; 1] {
        [Self::ForceQmi]
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
    /// Retained WDS client id. DATA6 requires every follow-up operation to use
    /// the same CID that started the packet-data session.
    pub client_id: String,
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
    /// Data-plane interface paired with DATA6, e.g. `wwan1`.
    pub netdev: Option<String>,
    /// Open mode proven to work by the capability probe.
    pub open_mode: QmiOpenMode,
    /// rpmsg driver backing this endpoint. Empty for pre-existing endpoints.
    pub driver: String,
    /// Whether this module bound the channel (and so should unbind it).
    pub owned: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RuntimeEndpointState {
    baseband: String,
    channel: String,
    port_name: String,
    device_path: String,
    netdev: Option<String>,
    driver: String,
}

enum RuntimeEndpointMapLookup {
    Missing,
    NoMatch,
    Found(RuntimeEndpointState),
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
    /// A prepared endpoint disappeared or was replaced while the initializer
    /// was holding it for ModemManager.
    EndpointLost(String),
}

impl std::fmt::Display for SecondaryQmiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(d) => write!(f, "secondary_qmi_unsupported:{d}"),
            Self::PrimaryUnresolved(d) => write!(f, "secondary_qmi_primary_unresolved:{d}"),
            Self::NoChannelAvailable(d) => write!(f, "secondary_qmi_no_channel:{d}"),
            Self::BindFailed(d) => write!(f, "secondary_qmi_bind_failed:{d}"),
            Self::ProbeFailed(d) => write!(f, "secondary_qmi_probe_failed:{d}"),
            Self::EndpointLost(d) => write!(f, "secondary_qmi_endpoint_lost:{d}"),
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

/// Wait for the basebands to publish their primary QMI control ports.
///
/// `secondary-qmi-init` is ordered before ModemManager, which puts it well ahead
/// of the modem: on this hardware the firmware finishes booting and attaches its
/// wwan ports around 13 s after kernel start, and the rpmsg channels land later
/// still. Enumerating immediately therefore finds nothing and the whole DATA6
/// preparation is skipped for the rest of the boot.
///
/// `udevadm settle` cannot substitute for this. It waits for the *event queue* to
/// drain and returns at once when the device does not exist yet, which is exactly
/// the situation here.
///
/// Once a port shows up this keeps polling until the count holds steady for
/// [`PRIMARY_PORT_SETTLE`]. A host with two modems can attach them seconds apart,
/// and returning on the first one would silently leave the second baseband
/// without an IMS endpoint.
pub async fn wait_for_primary_qmi_ports(timeout: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut ports = discover_primary_qmi_ports();
    let mut stable_since = None;

    while tokio::time::Instant::now() < deadline {
        if !ports.is_empty() {
            match stable_since {
                Some(since) if tokio::time::Instant::now().duration_since(since) >= PRIMARY_PORT_SETTLE => {
                    break;
                }
                Some(_) => {}
                None => stable_since = Some(tokio::time::Instant::now()),
            }
        }
        tokio::time::sleep(PORT_POLL_INTERVAL).await;
        let latest = discover_primary_qmi_ports();
        if latest != ports {
            // Something changed: restart the settle window.
            stable_since = (!latest.is_empty()).then(tokio::time::Instant::now);
            ports = latest;
        }
    }

    ports
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

/// Beta8 only allocates DATA6. Other DATA channels can belong to firmware or
/// recovery paths and must not be used as speculative fallbacks.
pub fn is_spare_data_channel(channel: &str) -> bool {
    channel == SECONDARY_CHANNEL
}

/// Ports currently published for a baseband, with their advertised type.
fn ports_for_baseband(baseband: &str) -> Vec<(String, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(WWAN_CLASS_DIR) else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_wwan_port_entry(&entry, &name) {
            continue;
        }
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

fn driver_bind_path(driver: &str) -> PathBuf {
    PathBuf::from(RPMSG_DRIVERS_DIR).join(driver).join("bind")
}

fn driver_unbind_path(driver: &str) -> PathBuf {
    PathBuf::from(RPMSG_DRIVERS_DIR).join(driver).join("unbind")
}

/// Drivers present on this host, in preference order. The custom multi driver is
/// preferred because it types spare channels as real QMI ports.
fn stock_driver_available() -> bool {
    driver_bind_path(RPMSG_WWAN_DRIVER).exists()
}

/// Is this driver one of ours (i.e. safe to rebind a channel away from)?
fn is_wwan_ctrl_driver(driver: &str) -> bool {
    driver == RPMSG_WWAN_DRIVER || driver == LEGACY_RPMSG_WWAN_DRIVER
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
/// The MSM8916/Beta8 contract is intentionally narrow: locate DATA6 on the same
/// baseband as `primary_device`, reuse it only when it is already bound to the
/// stock driver, or bind that one channel and verify the one port it creates.
pub async fn ensure_endpoint(
    primary_device: &str,
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    if !secondary_qmi_enabled() {
        return Err(SecondaryQmiError::Unsupported(
            "DATA6 probing requires SIMADMIN_ENABLE_SECONDARY_QMI=1".to_string(),
        ));
    }
    let baseband = baseband_key_for_device(primary_device)?;
    let primary_port = primary_device
        .rsplit('/')
        .next()
        .unwrap_or(primary_device)
        .to_string();

    if !stock_driver_available() {
        return Err(SecondaryQmiError::Unsupported(format!(
            "stock RPMSG WWAN driver {RPMSG_WWAN_DRIVER} is unavailable"
        )));
    }
    let channels = enumerate_channels()?;
    let candidate = channels
        .iter()
        .filter(|channel| channel.baseband == baseband)
        .find(|channel| channel.channel == SECONDARY_CHANNEL)
        .ok_or_else(|| {
            SecondaryQmiError::NoChannelAvailable(format!(
                "{SECONDARY_CHANNEL} RPMSG device is unavailable under {baseband}"
            ))
        })?;

    if let Some(bound) = candidate.bound_driver.as_deref() {
        if !is_wwan_ctrl_driver(bound) {
            return Err(SecondaryQmiError::NoChannelAvailable(format!(
                "{SECONDARY_CHANNEL} is already bound to unrelated driver {bound}"
            )));
        }
        if bound == RPMSG_WWAN_DRIVER {
            return reuse_bound_data6(candidate, &baseband, &primary_port).await;
        }
    }

    bind_and_probe(candidate, &baseband, &primary_port).await
}

/// Resolve the DATA6 endpoint published by the boot initializer.
///
/// Beta8 reads `SIMADMIN_SECONDARY_QMI_DEVICE` first and otherwise treats
/// `/run/simadmin/secondary-qmi-device` as a plain device path. Runtime bearer
/// activation must prefer that held endpoint instead of rebinding/probing
/// DATA6 while the initializer and ModemManager are already running.
pub async fn runtime_endpoint(
    primary_device: &str,
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    if !secondary_qmi_enabled() {
        return Err(SecondaryQmiError::Unsupported(
            "DATA6 is disabled by default; set SIMADMIN_ENABLE_SECONDARY_QMI=1 after firmware validation".to_string(),
        ));
    }
    if let Some(endpoint) = endpoint_from_runtime_state(primary_device)? {
        return Ok(endpoint);
    }
    Err(SecondaryQmiError::Unsupported(
        "DATA6 was not prepared by the opt-in secondary-QMI initializer".to_string(),
    ))
}

/// Report whether the boot initializer published a valid secondary endpoint
/// for this exact baseband.  This never binds or probes a channel.
pub fn runtime_endpoint_available(primary_device: &str) -> bool {
    secondary_qmi_enabled()
        && endpoint_from_runtime_state(primary_device)
            .ok()
            .flatten()
            .is_some()
}

fn endpoint_from_runtime_state(
    primary_device: &str,
) -> Result<Option<SecondaryQmiEndpoint>, SecondaryQmiError> {
    let primary_baseband = baseband_key_for_device(primary_device)?;
    let configured = match std::env::var("SIMADMIN_SECONDARY_QMI_DEVICE") {
        Ok(value) if !value.trim().is_empty() => Some(legacy_runtime_endpoint_state(value, None)),
        _ => match endpoint_from_runtime_map(&primary_baseband)? {
            RuntimeEndpointMapLookup::Found(endpoint) => {
                return configured_runtime_endpoint(primary_baseband, endpoint);
            }
            RuntimeEndpointMapLookup::NoMatch => return Ok(None),
            RuntimeEndpointMapLookup::Missing => {
                match std::fs::read_to_string(SECONDARY_QMI_STATE_FILE) {
                    Ok(value) => {
                        // Accept the short-lived JSON format written by older Codex
                        // builds so an in-place upgrade can recover without a reboot.
                        let trimmed = value.trim();
                        if trimmed.starts_with('{') {
                            let parsed: serde_json::Value =
                                serde_json::from_str(trimmed).map_err(|error| {
                                    SecondaryQmiError::ProbeFailed(format!(
                                        "invalid {SECONDARY_QMI_STATE_FILE}: {error}"
                                    ))
                                })?;
                            let device = parsed
                                .get("qmi_device")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let netdev = parsed
                                .get("netdev")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string);
                            Some(legacy_runtime_endpoint_state(device, netdev))
                        } else {
                            Some(legacy_runtime_endpoint_state(trimmed.to_string(), None))
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err(SecondaryQmiError::ProbeFailed(format!(
                            "failed to read {SECONDARY_QMI_STATE_FILE}: {error}"
                        )))
                    }
                }
            }
        },
    };
    let Some(state) = configured else {
        return Ok(None);
    };
    configured_runtime_endpoint(primary_baseband, state)
}

fn legacy_runtime_endpoint_state(
    device_path: String,
    netdev: Option<String>,
) -> RuntimeEndpointState {
    RuntimeEndpointState {
        baseband: String::new(),
        channel: SECONDARY_CHANNEL.to_string(),
        port_name: String::new(),
        device_path,
        netdev,
        driver: RPMSG_WWAN_DRIVER.to_string(),
    }
}

fn endpoint_from_runtime_map(
    primary_baseband: &str,
) -> Result<RuntimeEndpointMapLookup, SecondaryQmiError> {
    let payload = match std::fs::read_to_string(SECONDARY_QMI_ENDPOINTS_STATE_FILE) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimeEndpointMapLookup::Missing)
        }
        Err(error) => {
            return Err(SecondaryQmiError::ProbeFailed(format!(
                "failed to read {SECONDARY_QMI_ENDPOINTS_STATE_FILE}: {error}"
            )))
        }
    };
    Ok(
        match runtime_endpoint_for_baseband(&payload, primary_baseband)? {
            Some(endpoint) => RuntimeEndpointMapLookup::Found(endpoint),
            None => RuntimeEndpointMapLookup::NoMatch,
        },
    )
}

fn runtime_endpoint_for_baseband(
    payload: &str,
    primary_baseband: &str,
) -> Result<Option<RuntimeEndpointState>, SecondaryQmiError> {
    let endpoints: Vec<RuntimeEndpointState> = serde_json::from_str(payload).map_err(|error| {
        SecondaryQmiError::ProbeFailed(format!(
            "invalid {SECONDARY_QMI_ENDPOINTS_STATE_FILE}: {error}"
        ))
    })?;
    Ok(endpoints
        .into_iter()
        .find(|endpoint| endpoint.baseband == primary_baseband))
}

fn configured_runtime_endpoint(
    primary_baseband: String,
    state: RuntimeEndpointState,
) -> Result<Option<SecondaryQmiEndpoint>, SecondaryQmiError> {
    let RuntimeEndpointState {
        channel,
        port_name,
        device_path,
        netdev,
        driver,
        ..
    } = state;
    let device_path = device_path.trim().to_string();
    if !device_path.starts_with("/dev/") || !Path::new(&device_path).exists() {
        return Err(SecondaryQmiError::ProbeFailed(format!(
            "configured secondary QMI endpoint is unavailable: {device_path}"
        )));
    }

    let secondary_baseband = baseband_key_for_device(&device_path)?;
    if secondary_baseband != primary_baseband {
        return Err(SecondaryQmiError::ProbeFailed(format!(
            "configured secondary QMI endpoint belongs to {secondary_baseband}, expected {primary_baseband}"
        )));
    }
    let port_name = if port_name.trim().is_empty() {
        device_path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        port_name
    };
    let netdev = std::env::var("SIMADMIN_SECONDARY_QMI_NETDEV")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or(netdev);
    Ok(Some(SecondaryQmiEndpoint {
        remoteproc: primary_baseband,
        rpmsg_device: String::new(),
        channel,
        port_name,
        device_path,
        netdev,
        open_mode: QmiOpenMode::ForceQmi,
        driver,
        owned: false,
    }))
}

/// Boot-time ports that are never the secondary endpoint (the modem's own AT
/// consoles). Everything else is a candidate and gets probed.
fn is_boot_port(port: &str) -> bool {
    port.ends_with("at0") || port.ends_with("at1")
}

fn uevent_marks_wwan_port(uevent: &str) -> bool {
    uevent
        .lines()
        .any(|line| line.trim() == "DEVTYPE=wwan_port")
}

fn is_wwan_port_entry(entry: &std::fs::DirEntry, name: &str) -> bool {
    std::fs::read_to_string(entry.path().join("uevent"))
        .map(|uevent| uevent_marks_wwan_port(&uevent))
        // Older WWAN class implementations may omit DEVTYPE. A real control
        // port still has a matching character node; the parent wwan_dev does not.
        .unwrap_or_else(|_| PathBuf::from("/dev").join(name).exists())
}

fn ports_for_rpmsg_device(device_id: &str) -> Vec<String> {
    let device = PathBuf::from(RPMSG_DEVICES_DIR).join(device_id);
    let Ok(device) = std::fs::canonicalize(device) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(WWAN_CLASS_DIR) else {
        return Vec::new();
    };
    let mut ports = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_wwan_port_entry(&entry, &name) {
                return None;
            }
            let resolved = std::fs::canonicalize(entry.path()).ok()?;
            resolved.starts_with(&device).then_some(name)
        })
        .collect::<Vec<_>>();
    ports.sort();
    ports
}

fn netdevs_for_baseband(baseband: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(NET_CLASS_DIR) else {
        return Vec::new();
    };
    let mut netdevs = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" {
                return None;
            }
            let resolved = std::fs::canonicalize(entry.path()).ok()?;
            (remoteproc_of_path(&resolved.to_string_lossy()).as_deref() == Some(baseband))
                .then_some(name)
        })
        .collect::<Vec<_>>();
    netdevs.sort();
    netdevs
}

fn choose_data6_netdev(
    netdevs: &[String],
    netdevs_before: &[String],
    primary_port: &str,
) -> Option<String> {
    let primary_netdev = primary_port.split("qmi").next().unwrap_or(primary_port);
    netdevs
        .iter()
        .find(|name| !netdevs_before.contains(name) && name.as_str() != primary_netdev)
        .or_else(|| netdevs.iter().find(|name| name.as_str() != primary_netdev))
        .cloned()
}

async fn prepare_data6_netdev(
    baseband: &str,
    primary_port: &str,
    netdevs_before: &[String],
) -> Result<String, SecondaryQmiError> {
    let netdev = std::env::var("SIMADMIN_SECONDARY_QMI_NETDEV")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            choose_data6_netdev(
                &netdevs_for_baseband(baseband),
                netdevs_before,
                primary_port,
            )
        })
        .ok_or_else(|| {
            SecondaryQmiError::BindFailed(format!("no DATA6 netdev appeared under {baseband}"))
        })?;

    // Binding DATA6 must not issue an administrative OPEN. On MSM8916 the
    // bam-dmux firmware treats an OPEN without an active WDS session as a
    // data-plane operation; during remoteproc recovery it can crash in
    // dhcp_client_mgr/smd_dsm_memcpy and wedge every WWAN netdev. The actual
    // IMS/data session setup calls qmi_netdev::resolve, which opens only the
    // selected interface after it has a valid address. Here we only resolve
    // and retain the netdev identity.
    Ok(netdev)
}

async fn reuse_bound_data6(
    candidate: &RpmsgChannel,
    baseband: &str,
    primary_port: &str,
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    let mut ports = ports_for_rpmsg_device(&candidate.device_id);
    if ports.is_empty() {
        ports = ports_for_baseband(baseband)
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name != primary_port && !is_boot_port(name))
            .collect();
    }
    if ports.len() != 1 {
        return Err(SecondaryQmiError::BindFailed(format!(
            "DATA6 is bound to the stock driver but its WWAN port is {}",
            if ports.is_empty() {
                "unknown".to_string()
            } else {
                format!("ambiguous ({})", ports.join(","))
            }
        )));
    }
    let port = ports.remove(0);
    let device_path = format!("/dev/{port}");
    if !Path::new(&device_path).exists() {
        return Err(SecondaryQmiError::BindFailed(format!(
            "DATA6 WWAN node is absent: {device_path}"
        )));
    }
    let netdev = prepare_data6_netdev(baseband, primary_port, &[]).await?;
    let open_mode = probe_qmi_capability(&device_path).await.ok_or_else(|| {
        SecondaryQmiError::ProbeFailed(format!(
            "{port} bound to {RPMSG_WWAN_DRIVER} does not expose the wds service"
        ))
    })?;
    Ok(SecondaryQmiEndpoint {
        remoteproc: baseband.to_string(),
        rpmsg_device: candidate.device_id.clone(),
        channel: candidate.channel.clone(),
        port_name: port,
        device_path,
        netdev: Some(netdev),
        open_mode,
        driver: RPMSG_WWAN_DRIVER.to_string(),
        owned: false,
    })
}

async fn bind_and_probe(
    candidate: &RpmsgChannel,
    baseband: &str,
    primary_port: &str,
) -> Result<SecondaryQmiEndpoint, SecondaryQmiError> {
    if candidate.bound_driver.as_deref() == Some(LEGACY_RPMSG_WWAN_DRIVER) {
        let old_ports = ports_for_rpmsg_device(&candidate.device_id);
        if let Some(bound) = candidate.bound_driver.as_deref() {
            let _ = std::fs::write(driver_unbind_path(bound), &candidate.device_id);
        }
        let deadline = tokio::time::Instant::now() + PORT_APPEAR_TIMEOUT;
        while tokio::time::Instant::now() < deadline
            && old_ports
                .iter()
                .any(|port| Path::new(&format!("/dev/{port}")).exists())
        {
            sleep(PORT_POLL_INTERVAL).await;
        }
        if old_ports
            .iter()
            .any(|port| Path::new(&format!("/dev/{port}")).exists())
        {
            return Err(SecondaryQmiError::BindFailed(format!(
                "old secondary QMI endpoint did not disappear: {}",
                old_ports.join(",")
            )));
        }
    }

    let ports_before: Vec<String> = ports_for_baseband(baseband)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let netdevs_before = netdevs_for_baseband(baseband);
    let device_dir = PathBuf::from(RPMSG_DEVICES_DIR).join(&candidate.device_id);
    std::fs::write(device_dir.join("driver_override"), RPMSG_WWAN_DRIVER).map_err(|e| {
        SecondaryQmiError::BindFailed(format!("driver_override {}: {e}", candidate.channel))
    })?;
    if let Err(error) = std::fs::write(driver_bind_path(RPMSG_WWAN_DRIVER), &candidate.device_id) {
        debug!(channel = %candidate.channel, error = %error, "bind write returned an error");
    }

    let deadline = tokio::time::Instant::now() + PORT_APPEAR_TIMEOUT;
    let mut discovered = None;
    while tokio::time::Instant::now() < deadline {
        let exact = ports_for_rpmsg_device(&candidate.device_id);
        let fresh: Vec<(String, Option<String>)> = ports_for_baseband(baseband)
            .into_iter()
            .filter(|(name, _)| !ports_before.contains(name))
            .collect();
        if exact.len() == 1 {
            let port = exact[0].clone();
            let port_type = fresh
                .iter()
                .find(|(name, _)| *name == port)
                .and_then(|(_, kind)| kind.clone());
            discovered = Some((port, port_type));
            break;
        }
        if exact.is_empty() && fresh.len() == 1 {
            discovered = fresh.into_iter().next();
            break;
        }
        sleep(PORT_POLL_INTERVAL).await;
    }

    let Some((port, port_type)) = discovered else {
        release_endpoint_by_device(&candidate.device_id, RPMSG_WWAN_DRIVER).await;
        return Err(SecondaryQmiError::BindFailed(format!(
            "{} bound via {} but no unique new port appeared under {baseband}",
            candidate.channel, RPMSG_WWAN_DRIVER,
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

    let netdev = match prepare_data6_netdev(baseband, primary_port, &netdevs_before).await {
        Ok(netdev) => netdev,
        Err(error) => {
            release_endpoint_by_device(&candidate.device_id, RPMSG_WWAN_DRIVER).await;
            return Err(error);
        }
    };

    match probe_qmi_capability(&device_path).await {
        Some(open_mode) => {
            info!(
                baseband = %baseband,
                channel = %candidate.channel,
                driver = %RPMSG_WWAN_DRIVER,
                port = %port,
                netdev = %netdev,
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
                netdev: Some(netdev),
                open_mode,
                driver: RPMSG_WWAN_DRIVER.to_string(),
                owned: true,
            })
        }
        None => {
            release_endpoint_by_device(&candidate.device_id, RPMSG_WWAN_DRIVER).await;
            Err(SecondaryQmiError::ProbeFailed(format!(
                "{port} (type {port_type:?}, driver {RPMSG_WWAN_DRIVER}) does not expose the wds service"
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    canonical_path: PathBuf,
    file_type: std::fs::FileType,
}

fn endpoint_identity(path: &Path) -> std::io::Result<EndpointIdentity> {
    let metadata = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(EndpointIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            file_type: metadata.file_type(),
        })
    }
    #[cfg(not(unix))]
    {
        Ok(EndpointIdentity {
            canonical_path: std::fs::canonicalize(path)?,
            file_type: metadata.file_type(),
        })
    }
}

/// Keep the boot initializer alive after DATA6 is ready. Beta8 records the
/// device identity and polls it; ModemManager is allowed to start only while the
/// same character node remains present.
pub async fn hold_endpoint(endpoint: &SecondaryQmiEndpoint) -> Result<(), SecondaryQmiError> {
    let path = Path::new(&endpoint.device_path);
    // Keeping the character device open is part of Beta8's DATA6 contract. The
    // stock rpmsg_wwan_ctrl driver tears down retained QMI client ids when its
    // last file descriptor closes, even when qmicli used
    // --client-no-release-cid. A passive metadata monitor is therefore not
    // enough: one long-lived O_RDWR descriptor must survive across all qmicli
    // invocations made by the runtime.
    #[cfg(unix)]
    let _held_device = {
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    };
    #[cfg(not(unix))]
    let _held_device = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path);
    let _held_device = _held_device.map_err(|error| {
        SecondaryQmiError::EndpointLost(format!(
            "failed to hold secondary QMI endpoint {} open: {error}",
            endpoint.device_path
        ))
    })?;

    let expected = endpoint_identity(path).map_err(|error| {
        SecondaryQmiError::EndpointLost(format!(
            "failed to inspect secondary QMI endpoint {}: {error}",
            endpoint.device_path
        ))
    })?;

    loop {
        sleep(Duration::from_secs(3)).await;
        match endpoint_identity(path) {
            Ok(current) if current == expected => {}
            Ok(_) => {
                return Err(SecondaryQmiError::EndpointLost(format!(
                    "secondary QMI endpoint was replaced: {}",
                    endpoint.device_path
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SecondaryQmiError::EndpointLost(format!(
                    "secondary QMI endpoint disappeared: {}",
                    endpoint.device_path
                )));
            }
            Err(error) => {
                return Err(SecondaryQmiError::EndpointLost(format!(
                    "failed to inspect secondary QMI endpoint {}: {error}",
                    endpoint.device_path
                )));
            }
        }
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

    let client_id = allocate_wds_client(endpoint).await?;
    let family_action = format!("--wds-set-ip-family={family}");
    if let Err(error) = run_retained_wds_action(endpoint, &client_id, &family_action).await {
        release_wds_client(endpoint, &client_id).await;
        return Err(error);
    }
    let output = match run_retained_wds_action(endpoint, &client_id, &start).await {
        Ok(output) => output,
        Err(error) => {
            release_wds_client(endpoint, &client_id).await;
            return Err(error);
        }
    };

    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let handle = match parse_packet_data_handle(&text) {
        Some(handle) => handle,
        None => {
            // Surface the modem's own verbose reason: it distinguishes "network wants
            // the other family" from a real failure.
            let reason = parse_call_end_reason(&text).unwrap_or_else(|| text.trim().to_string());
            release_wds_client(endpoint, &client_id).await;
            return Err(format!("secondary_qmi_start_failed:{reason}"));
        }
    };

    let settings = read_current_settings(endpoint, &client_id)
        .await
        .unwrap_or_default();
    Ok(ImsSession {
        client_id,
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

async fn allocate_wds_client(endpoint: &SecondaryQmiEndpoint) -> Result<String, String> {
    let output = run_qmicli(&[
        "--verbose",
        "-d",
        &endpoint.device_path,
        "--device-open-qmi",
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        "--client-no-release-cid",
        "--wds-noop",
    ])
    .await
    .ok_or_else(|| "secondary_qmi_cid_allocate_spawn_failed".to_string())?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!("secondary_qmi_cid_allocate_failed:{}", text.trim()));
    }
    parse_wds_client_id(&text).ok_or_else(|| format!("secondary_qmi_cid_missing:{}", text.trim()))
}

async fn run_retained_wds_action(
    endpoint: &SecondaryQmiEndpoint,
    client_id: &str,
    action: &str,
) -> Result<Output, String> {
    let cid = format!("--client-cid={client_id}");
    let output = run_qmicli(&[
        "-d",
        &endpoint.device_path,
        "--device-open-qmi",
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        &cid,
        "--client-no-release-cid",
        action,
    ])
    .await
    .ok_or_else(|| "secondary_qmi_action_spawn_failed".to_string())?;
    if output.status.success() {
        Ok(output)
    } else {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Err(format!("secondary_qmi_action_failed:{}", text.trim()))
    }
}

async fn release_wds_client(endpoint: &SecondaryQmiEndpoint, client_id: &str) {
    let cid = format!("--client-cid={client_id}");
    let _ = run_qmicli(&[
        "-d",
        &endpoint.device_path,
        "--device-open-qmi",
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        &cid,
        "--wds-noop",
    ])
    .await;
}

/// Tear down an IMS session and release the retained WDS CID.
pub async fn stop_ims_session(endpoint: &SecondaryQmiEndpoint, session: &ImsSession) {
    let stop = format!("--wds-stop-network={}", session.packet_data_handle);
    let _ = run_retained_wds_action(endpoint, &session.client_id, &stop).await;
    release_wds_client(endpoint, &session.client_id).await;
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

async fn read_current_settings(
    endpoint: &SecondaryQmiEndpoint,
    client_id: &str,
) -> Option<CurrentSettings> {
    let output = run_retained_wds_action(endpoint, client_id, "--wds-get-current-settings")
        .await
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(parse_current_settings(&text))
}

/// Parse the WDS CID printed by qmicli's retained-client output or verbose log.
pub fn parse_wds_client_id(text: &str) -> Option<String> {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("registered 'wds'") {
            if let Some(value) = line
                .split_once("client with ID '")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(value, _)| value)
            {
                if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
                    return Some(value.to_string());
                }
            }
        }
        if lower.contains("service = 'wds'") {
            if let Some(value) = line
                .split_once("cid = '")
                .and_then(|(_, rest)| rest.split_once('\''))
                .map(|(value, _)| value)
            {
                if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
                    return Some(value.to_string());
                }
            }
        }
        if let Some(index) = lower.find("cid:") {
            let value = line[index + 4..].trim().trim_matches('\'').trim();
            if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
                return Some(value.to_string());
            }
        }
    }
    None
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
    fn runtime_endpoint_map_selects_matching_baseband() {
        let payload = r#"[
            {"baseband":"4080000.remoteproc","channel":"DATA6_CNTL","port_name":"wwan0qmi1","device_path":"/dev/wwan0qmi1","netdev":"wwan1","driver":"rpmsg_wwan_ctrl"},
            {"baseband":"6080000.remoteproc","channel":"DATA6_CNTL","port_name":"wwan2qmi1","device_path":"/dev/wwan2qmi1","netdev":"wwan3","driver":"rpmsg_wwan_ctrl"}
        ]"#;

        let endpoint = runtime_endpoint_for_baseband(payload, "6080000.remoteproc")
            .unwrap()
            .unwrap();
        assert_eq!(endpoint.device_path, "/dev/wwan2qmi1");
        assert_eq!(endpoint.netdev.as_deref(), Some("wwan3"));
    }

    #[test]
    fn runtime_endpoint_map_does_not_fall_back_to_first_baseband() {
        let payload = r#"[
            {"baseband":"4080000.remoteproc","channel":"DATA6_CNTL","port_name":"wwan0qmi1","device_path":"/dev/wwan0qmi1","netdev":null,"driver":"rpmsg_wwan_ctrl"}
        ]"#;

        assert!(runtime_endpoint_for_baseband(payload, "6080000.remoteproc")
            .unwrap()
            .is_none());
    }

    #[test]
    fn only_data6_cntl_is_eligible() {
        assert!(is_spare_data_channel("DATA6_CNTL"));
        assert!(!is_spare_data_channel("DATA5_CNTL"));
        assert!(!is_spare_data_channel("DATA7_CNTL"));
        assert!(!is_spare_data_channel("DATA40_CNTL"));
        assert!(!is_spare_data_channel("DIAG_CNTL"));
        assert!(!is_spare_data_channel("DATA1"));
    }

    #[test]
    fn data6_netdev_prefers_a_fresh_non_primary_interface() {
        let all = vec![
            "wwan0".to_string(),
            "wwan1".to_string(),
            "wwan2".to_string(),
        ];
        let before = vec!["wwan0".to_string(), "wwan1".to_string()];
        assert_eq!(
            choose_data6_netdev(&all, &before, "wwan0qmi0").as_deref(),
            Some("wwan2")
        );
        assert_eq!(
            choose_data6_netdev(&all[..2], &before, "wwan0qmi0").as_deref(),
            Some("wwan1")
        );
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
    fn wwan_parent_is_not_a_control_port() {
        assert!(!uevent_marks_wwan_port("DEVTYPE=wwan_dev\n"));
        assert!(uevent_marks_wwan_port(
            "MAJOR=242\nMINOR=3\nDEVNAME=wwan0at2\nDEVTYPE=wwan_port\n"
        ));
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
        assert_eq!(QmiOpenMode::probe_order(), [QmiOpenMode::ForceQmi]);
    }

    #[test]
    fn beta8_uses_stock_driver_and_only_migrates_the_legacy_driver() {
        assert_eq!(RPMSG_WWAN_DRIVER, "rpmsg_wwan_ctrl");
        assert_eq!(LEGACY_RPMSG_WWAN_DRIVER, "rpmsg_wwan_ctrl_multi");
        assert!(is_wwan_ctrl_driver("rpmsg_wwan_ctrl_multi"));
        assert!(is_wwan_ctrl_driver("rpmsg_wwan_ctrl"));
        // A channel held by an unrelated driver must not be stolen.
        assert!(!is_wwan_ctrl_driver("rpmsg_chrdev"));
        assert!(!is_wwan_ctrl_driver("qcom_smd_qrtr"));
    }

    /// The port wait lives in the binary, but the deadline that can cut it short
    /// lives in the systemd unit. They are edited in different files, so pin the
    /// relationship: a `TimeoutStartSec` below the wait means systemd kills the
    /// initializer mid-probe and DATA6 is skipped for the rest of the boot.
    #[test]
    fn the_unit_allows_more_startup_time_than_the_port_wait_needs() {
        let unit = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../deploy/system/simadmin-secondary-qmi.service"),
        )
        .expect("the packaged unit must exist");

        let timeout: u64 = unit
            .lines()
            .find_map(|line| line.trim().strip_prefix("TimeoutStartSec="))
            .expect("the unit must set TimeoutStartSec")
            .trim()
            .parse()
            .expect("TimeoutStartSec must be plain seconds");

        assert!(
            timeout > PRIMARY_PORT_WAIT.as_secs(),
            "TimeoutStartSec={timeout}s must exceed the {}s port wait",
            PRIMARY_PORT_WAIT.as_secs()
        );

        // No *directive* may name a channel, driver, or port: the whole point of
        // discovering the layout at runtime is that these differ per platform,
        // and a hardcoded name once skipped the unit entirely. Comments are
        // exempt on purpose -- they carry the history of why that is banned.
        let directives: String = unit
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");

        for forbidden in ["DATA6_CNTL", "wwan0qmi1", "wwan0at2", "rpmsg_wwan_ctrl_multi"] {
            assert!(
                !directives.contains(forbidden),
                "the unit must not hardcode {forbidden}"
            );
        }
        assert!(
            !directives.contains("ExecCondition="),
            "an ExecCondition can skip the unit, and with it the legacy-module purge"
        );
    }

    /// The settle window exists to catch a second baseband attaching late; it is
    /// useless if it is not comfortably shorter than the overall wait.
    #[test]
    fn the_settle_window_fits_inside_the_port_wait() {
        assert!(PRIMARY_PORT_SETTLE < PRIMARY_PORT_WAIT);
        assert!(PORT_POLL_INTERVAL < PRIMARY_PORT_SETTLE);
    }

    /// The purge has to reach every location an older install could have used,
    /// because a module left loaded keeps auto-binding spare DATA*_CNTL
    /// channels at boot and that is what crashes the DSP.
    #[test]
    fn legacy_module_purge_covers_every_known_install_location() {
        let expanded: Vec<String> = LEGACY_MODULE_FILES
            .iter()
            .map(|t| t.replace("{kver}", "6.17.0-rc6-lkiuyu-compile+"))
            .collect();

        // Where beta8 kept it, and where deploy/install.sh puts it.
        assert!(expanded
            .iter()
            .any(|p| p == "/opt/simadmin/modules/rpmsg_wwan_ctrl_multi.ko"));
        assert!(expanded.iter().any(|p| p
            == "/lib/modules/6.17.0-rc6-lkiuyu-compile+/extra/simadmin/rpmsg_wwan_ctrl_multi.ko"));

        // Every entry must name the legacy module and no template may be left
        // unexpanded, or the removal silently misses.
        for path in &expanded {
            assert!(path.ends_with("rpmsg_wwan_ctrl_multi.ko"), "{path}");
            assert!(!path.contains("{kver}"), "{path}");
        }
        // The purge must never target the in-tree driver.
        assert!(!expanded.iter().any(|p| p.contains("rpmsg_wwan_ctrl.ko")));
        assert_eq!(LEGACY_MODULE_SYSFS, "/sys/module/rpmsg_wwan_ctrl_multi");
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
