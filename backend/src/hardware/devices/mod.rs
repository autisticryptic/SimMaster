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
    // The 410 exposes its modem DSP as 4080000.remoteproc.  Do not classify
    // the neighbouring a204000.remoteproc (WCNSS Wi-Fi/BT) as a baseband.
    if std::path::Path::new("/sys/devices/platform/soc@0/4080000.remoteproc").exists()
        || std::fs::read_dir("/sys/class/remoteproc")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| {
                std::fs::read_to_string(entry.path().join("name"))
                    .map(|name| name.trim() == "4080000.remoteproc")
                    .unwrap_or(false)
            })
    {
        return DeviceKind::Qcm410;
    }
    DeviceKind::Unknown
}
