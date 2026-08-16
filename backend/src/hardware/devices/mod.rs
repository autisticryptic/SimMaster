//! Device abstraction layer: per-device drivers keyed by device name.
//!
//! Upper protocol layers (user-space IMS, data, SMS, registration) talk to
//! [`transport`] traits, never to a concrete device. A device driver implements
//! those traits and registers here; dispatch picks the right driver at runtime
//! (sysfs detection, overridable by configuration).

pub mod pcsc;
pub mod qcm410;
pub mod quectel;
pub mod transport;

/// Enumerated device kinds known to SimAdmin.
///
/// `Unknown` keeps dispatch total even when the running platform is not (yet)
/// recognized, so upper layers can fall back to generic ModemManager behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// Qualcomm 410 (MSM8916-class) pocket-WiFi.
    Qcm410,
    /// A platform not (yet) covered by a dedicated driver.
    Unknown,
}

/// Resolve the device kind from the running platform.
///
/// Detection is best-effort sysfs inspection; an explicit configuration value
/// (not yet wired) would take precedence. `Unknown` is the safe fallback.
pub fn detect_device_kind() -> DeviceKind {
    // TODO: read /sys/devices/platform for `4080000.remoteproc` (baseband) and
    // friends to tell qcm410 from other Qualcomm/UNISOC/Quectel platforms.
    DeviceKind::Unknown
}
