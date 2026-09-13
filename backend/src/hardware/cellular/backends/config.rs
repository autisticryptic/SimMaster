//! Startup-only backend selection. Merely keeping a native device description
//! in configuration never probes it or changes the owner of a modem.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    #[default]
    Modemmanager,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeProtocol {
    Qmi,
    Mbim,
    At,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    #[serde(default)]
    pub mode: BackendMode,
    /// Deliberate maintenance-window opt-in, never enabled by an HTTP retry.
    #[serde(default)]
    pub allow_unvalidated_native: bool,
    #[serde(default)]
    pub devices: Vec<NativeDeviceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDeviceConfig {
    /// Stable physical anchor, copied from the existing slot mapping when
    /// migrating. It is not an IMEI, SIM identity or transient /dev number.
    pub hardware_key: String,
    /// Canonical sysfs ancestor to which ALL configured control ports belong.
    pub sysfs_anchor: String,
    pub protocol: NativeProtocol,
    pub control_device: String,
    #[serde(default)]
    pub at_device: Option<String>,
    #[serde(default = "slot_one")]
    pub uim_slot: u8,
    #[serde(default)]
    pub ims: Option<NativeBearerConfig>,
    #[serde(default)]
    pub data: Option<NativeBearerConfig>,
}

fn slot_one() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBearerConfig {
    pub control_device: String,
    /// A dedicated interface, never selected by "first wwan interface".
    pub interface: String,
    #[serde(default)]
    pub session_id: u8,
    /// Legacy QMI data-port binding used by BAM-DMUX controllers. This is
    /// separate from QMAP mux binding and must come from device evidence.
    #[serde(default)]
    pub qmi_data_port: Option<String>,
    /// Optional explicit QMI endpoint binding. No guessed BAM/USB mux layout.
    #[serde(default)]
    pub qmi_binding: Option<QmiEndpointBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QmiEndpointBinding {
    pub endpoint_type: String,
    pub interface_number: u32,
    pub mux_id: u8,
}

pub fn valid_device_path(value: &str) -> bool {
    value.starts_with("/dev/")
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && std::path::Path::new(value).components().all(|c| {
            matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

pub fn valid_interface(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 16
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        && value != "lo"
}

impl BackendConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.mode == BackendMode::Native && !self.allow_unvalidated_native {
            return Err("native_backend_requires_explicit_unvalidated_opt_in".into());
        }
        if self.mode == BackendMode::Native && self.devices.is_empty() {
            return Err("native_backend_requires_explicit_device_bindings".into());
        }
        if self.devices.len() > 32 {
            return Err("native_device_limit_exceeded".into());
        }
        let mut anchors = std::collections::HashSet::new();
        let mut ports = std::collections::HashSet::new();
        let mut interfaces = std::collections::HashSet::new();
        for device in &self.devices {
            device.validate()?;
            // One controller/active slot per physical modem in this candidate.
            // MEP must not be simulated by two controllers sharing RF/ports.
            if !anchors.insert(device.hardware_key.as_str()) {
                return Err("native_duplicate_physical_owner".into());
            }
            let own_ports = std::iter::once(device.control_device.as_str())
                .chain(device.at_device.as_deref())
                .chain(device.ims.iter().map(|e| e.control_device.as_str()))
                .chain(device.data.iter().map(|e| e.control_device.as_str()))
                .collect::<std::collections::HashSet<_>>();
            for port in own_ports {
                if !ports.insert(port) {
                    return Err("native_port_claimed_by_multiple_devices".into());
                }
            }
            for endpoint in device.ims.iter().chain(device.data.iter()) {
                if !interfaces.insert(endpoint.interface.as_str()) {
                    return Err("native_bearer_interface_not_exclusive".into());
                }
            }
        }
        Ok(())
    }
}

impl NativeDeviceConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.hardware_key.trim() != self.hardware_key
            || self.hardware_key.is_empty()
            || self.hardware_key.len() > 512
            || self.hardware_key.chars().any(char::is_control)
            || !self.sysfs_anchor.starts_with("/sys/devices/")
            || self.sysfs_anchor.trim_end_matches('/') == "/sys/devices"
            || self.sysfs_anchor.len() > 1024
            || !std::path::Path::new(&self.sysfs_anchor)
                .components()
                .all(|c| {
                    matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )
                })
            || self.sysfs_anchor.chars().any(char::is_control)
            || !valid_device_path(&self.control_device)
            || self
                .at_device
                .as_deref()
                .is_some_and(|p| !valid_device_path(p))
            || !(1..=8).contains(&self.uim_slot)
        {
            return Err("native_device_binding_invalid".into());
        }
        if self.protocol == NativeProtocol::At && (self.ims.is_some() || self.data.is_some()) {
            return Err("native_at_only_packet_data_requires_a_dedicated_driver".into());
        }
        if self.protocol != NativeProtocol::Qmi && self.uim_slot != 1 {
            return Err("native_mbim_at_multi_slot_requires_slot_mapping_driver".into());
        }
        for bearer in self.ims.iter().chain(self.data.iter()) {
            if !valid_device_path(&bearer.control_device) || !valid_interface(&bearer.interface) {
                return Err("native_bearer_binding_invalid".into());
            }
            if self.protocol != NativeProtocol::Qmi
                && (bearer.qmi_binding.is_some() || bearer.qmi_data_port.is_some())
            {
                return Err("native_qmi_binding_on_non_qmi_device".into());
            }
            if let Some(port) = &bearer.qmi_data_port {
                let suffix = port
                    .strip_prefix("a2-mux-rmnet")
                    .and_then(|s| s.parse::<u8>().ok());
                if !suffix.is_some_and(|n| n <= 7) {
                    return Err("native_qmi_data_port_invalid".into());
                }
            }
            if let Some(binding) = &bearer.qmi_binding {
                if !matches!(
                    binding.endpoint_type.as_str(),
                    "hsusb" | "pcie" | "embedded" | "bam-dmux"
                ) {
                    return Err("native_qmi_endpoint_type_invalid".into());
                }
            }
        }
        Ok(())
    }

    pub fn line_id(&self) -> String {
        crate::hardware::cellular::bindings::physical_line_id(&self.hardware_key, self.uim_slot)
    }

    pub fn selector(&self) -> String {
        format!("native:{}", self.line_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn device() -> NativeDeviceConfig {
        NativeDeviceConfig {
            hardware_key: "sysfs:slot-1".into(),
            sysfs_anchor: "/sys/devices/test-modem".into(),
            protocol: NativeProtocol::Qmi,
            control_device: "/dev/wwan0qmi0".into(),
            at_device: Some("/dev/wwan0at0".into()),
            uim_slot: 1,
            ims: None,
            data: None,
        }
    }

    #[test]
    fn defaults_remain_mm_and_native_is_never_an_automatic_fallback() {
        let config: BackendConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.mode, BackendMode::Modemmanager);
        assert!(config.validate().is_ok());
        let mut candidate = config;
        candidate.devices.push(device());
        assert!(
            candidate.validate().is_ok(),
            "parked descriptions are allowed"
        );
        candidate.mode = BackendMode::Native;
        assert!(candidate.validate().is_err());
        candidate.allow_unvalidated_native = true;
        assert!(candidate.validate().is_ok());
    }

    #[test]
    fn backend_switch_preserves_physical_line_identity() {
        assert_eq!(device().line_id(), "line-fa9b70acd51cf9273a726ab948073c92");
        assert!(device().selector().starts_with("native:line-"));
    }

    #[test]
    fn reject_shared_modems_ports_interfaces_and_unsafe_paths() {
        let mut config = BackendConfig::default();
        config.devices = vec![device(), device()];
        assert_eq!(
            config.validate().unwrap_err(),
            "native_duplicate_physical_owner"
        );
        config.devices[1].hardware_key = "different-slot".into();
        assert!(config.validate().is_err(), "same port is not another modem");
        for path in ["/etc/passwd", "/dev/../etc/passwd", "/dev/tty\nUSB0"] {
            assert!(!valid_device_path(path));
        }
        assert!(!valid_interface("lo"));
        assert!(!valid_interface("wwan0;reboot"));
    }
}
