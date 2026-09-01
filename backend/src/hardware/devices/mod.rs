//! Device abstraction layer: platform drivers selected from hardware evidence.
//!
//! Upper protocol layers (user-space IMS, data, SMS, registration) talk to
//! [`transport`] traits, never to a concrete device. A device driver implements
//! those traits and registers here; dispatch picks the right driver at runtime
//! from platform-owned hardware signatures.

use std::sync::Arc;

use crate::platform::config::ApnConfig;

use self::transport::{CellularDataTransport, ImsBearerTransport, TransportFuture};

pub mod baseband_faults;
pub mod pcsc;
pub mod qcm410;
pub mod quectel;
pub mod transport;

/// Enumerated device kinds known to SimAdmin.
///
/// `Unknown` keeps dispatch total even when the running platform is not (yet)
/// recognized. Its transports are deliberately unavailable: SimAdmin never
/// falls back to a bearer in the host network namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Qualcomm 410 (MSM8916-class) pocket-WiFi.
    Qcm410,
    /// A platform not (yet) covered by a dedicated driver.
    Unknown,
}

/// Hardware capabilities consumed by API and orchestration layers.
///
/// These facts describe the host device, not a carrier profile or a user's
/// routing preference. Keeping them on the driver prevents API handlers from
/// accumulating model checks as more hardware is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceCapabilities {
    pub gateway_mode: bool,
    pub local_audio_capable: bool,
    pub local_video_capable: bool,
}

/// Complete platform-driver boundary consumed by the application.
///
/// Adding another supported device requires one implementation in its own
/// module plus one detection/registry entry here. Protocol, service and OTA
/// code never imports the concrete driver.
pub trait DeviceDriver: Send + Sync {
    fn kind(&self) -> DeviceKind;

    /// Return whether this driver recognizes the running hardware.
    ///
    /// Platform signatures belong to the concrete driver so adding a device
    /// never adds sysfs paths or vendor identifiers to this registry.
    fn is_present(&self) -> bool;

    fn capabilities(&self) -> DeviceCapabilities;

    fn ims_bearer_transport(&self) -> Option<Arc<dyn ImsBearerTransport>>;

    fn cellular_data_transport(&self) -> Arc<dyn CellularDataTransport>;

    fn baseband_fault_policy(&self) -> &'static dyn baseband_faults::BasebandFaultPolicy;

    fn initialize_native_bearers(
        &self,
        write_udev_rule: bool,
        dry_run: bool,
    ) -> TransportFuture<'_, anyhow::Result<()>>;

    fn install_update_resources(&self, staging_dir: &str, restart_now: bool) -> String;
}

#[derive(Default)]
struct UnsupportedCellularDataTransport;

impl CellularDataTransport for UnsupportedCellularDataTransport {
    fn interface(&self) -> TransportFuture<'_, Option<String>> {
        Box::pin(async { None })
    }

    fn start<'a>(
        &'a self,
        _line_id: &'a str,
        _primary_device: &'a str,
        _apn: &'a ApnConfig,
    ) -> TransportFuture<'a, Result<String, String>> {
        Box::pin(async { Err("cellular_native_data_unsupported_for_device".to_string()) })
    }

    fn stop(&self) -> TransportFuture<'_, ()> {
        Box::pin(async {})
    }

    fn endpoint_available(&self, _primary_device: &str) -> bool {
        false
    }
}

struct UnsupportedDeviceDriver;

impl DeviceDriver for UnsupportedDeviceDriver {
    fn kind(&self) -> DeviceKind {
        DeviceKind::Unknown
    }

    fn is_present(&self) -> bool {
        false
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            gateway_mode: false,
            local_audio_capable: false,
            local_video_capable: false,
        }
    }

    fn ims_bearer_transport(&self) -> Option<Arc<dyn ImsBearerTransport>> {
        None
    }

    fn cellular_data_transport(&self) -> Arc<dyn CellularDataTransport> {
        Arc::new(UnsupportedCellularDataTransport)
    }

    fn baseband_fault_policy(&self) -> &'static dyn baseband_faults::BasebandFaultPolicy {
        &baseband_faults::GenericBasebandFaults
    }

    fn initialize_native_bearers(
        &self,
        _write_udev_rule: bool,
        _dry_run: bool,
    ) -> TransportFuture<'_, anyhow::Result<()>> {
        Box::pin(async {
            Err(anyhow::anyhow!(
                "native bearer initialization is unsupported for the detected device"
            ))
        })
    }

    fn install_update_resources(&self, _staging_dir: &str, _restart_now: bool) -> String {
        "no device-specific update resources required".to_string()
    }
}

static QCM410_DRIVER: qcm410::Driver = qcm410::Driver;
static UNSUPPORTED_DRIVER: UnsupportedDeviceDriver = UnsupportedDeviceDriver;

/// Return the registered driver for a detected platform.
pub fn driver(kind: DeviceKind) -> &'static dyn DeviceDriver {
    match kind {
        DeviceKind::Qcm410 => &QCM410_DRIVER,
        DeviceKind::Unknown => &UNSUPPORTED_DRIVER,
    }
}

/// Resolve the native IMS bearer provider for a detected device.
pub fn ims_bearer_transport(kind: DeviceKind) -> Option<Arc<dyn ImsBearerTransport>> {
    driver(kind).ims_bearer_transport()
}

/// Resolve the retained cellular-data provider for a detected device.
pub fn cellular_data_transport(kind: DeviceKind) -> Arc<dyn CellularDataTransport> {
    driver(kind).cellular_data_transport()
}

/// Resolve immutable host-device capabilities for API presentation and
/// orchestration policy.
pub fn capabilities(kind: DeviceKind) -> DeviceCapabilities {
    driver(kind).capabilities()
}

/// Resolve the baseband fault observer supplied by the device driver.
pub fn baseband_fault_policy(
    kind: DeviceKind,
) -> &'static dyn baseband_faults::BasebandFaultPolicy {
    driver(kind).baseband_fault_policy()
}

/// Dispatch the boot-time native-bearer preparation to the detected driver.
/// Device-specific channel, driver and udev logic must remain below this
/// boundary rather than leaking into the binary entry point.
pub async fn run_native_bearer_init(
    kind: DeviceKind,
    write_udev_rule: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    driver(kind)
        .initialize_native_bearers(write_udev_rule, dry_run)
        .await
}

/// Install update-package resources owned by the detected device driver.
///
/// The OTA service knows only the staging directory. Unit names, ordering and
/// activation details stay with the hardware implementation that requires them.
pub fn install_update_resources(kind: DeviceKind, staging_dir: &str, restart_now: bool) -> String {
    driver(kind).install_update_resources(staging_dir, restart_now)
}

/// Resolve the device kind from the running platform.
///
/// Detection is delegated to registered drivers. `Unknown` is the safe
/// fallback and never silently selects a concrete modem implementation.
pub fn detect_device_kind() -> DeviceKind {
    for candidate in [&QCM410_DRIVER as &dyn DeviceDriver] {
        if candidate.is_present() {
            return candidate.kind();
        }
    }
    DeviceKind::Unknown
}
