//! Read-only discovery of modem hardware for the native backend.
//!
//! Native mode needs an explicit device table ([`NativeDeviceConfig`]), and
//! writing one by hand means reading sysfs driver bindings, interface numbers
//! and control nodes. This module performs that walk and proposes a candidate.
//! It never opens a device node, never writes sysfs and never talks to a modem:
//! port roles are hints that an AT probe must still confirm, and IMS/data
//! bearer endpoints are left empty because their mapping needs device evidence.
//!
//! Capability comes from the kernel driver bound to each USB interface rather
//! than a vendor list: `qmi_wwan` exposes a QMI control node, `cdc_mbim` an
//! MBIM one, and `option` / `qcserial` the modem's serial ports. Quectel
//! modules in a serial/ECM/RNDIS composition bind none of the QMI/MBIM drivers,
//! so their vendor ID re-admits them; the first-generation DJI 4G module keeps
//! its own `2ca3:4006` identity, which no in-tree driver claims.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::config::{NativeDeviceConfig, NativeProtocol};

const QUECTEL_VENDOR: &str = "2c7c";
const DJI_VENDOR: &str = "2ca3";
const DJI_4G_PRODUCT: &str = "4006";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredPort {
    pub device: String,
    /// `/dev/serial/by-path` alias: follows the physical port across reboots,
    /// unlike the `ttyUSBn` number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_number: Option<u8>,
    pub driver: String,
    /// Conventional role only; an AT probe must confirm it before use.
    pub role_hint: &'static str,
}

impl DiscoveredPort {
    fn open_path(&self) -> &str {
        self.stable_path.as_deref().unwrap_or(&self.device)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredModem {
    /// `usb` for a USB composite device, `wwan` for the Linux WWAN class
    /// (PCIe/MHI modules and SoC-integrated basebands).
    pub bus: &'static str,
    /// Proposed sysfs-derived key. Compare with the existing line inventory:
    /// ModemManager may use a custom physical UID (for example `qcom-soc`).
    pub hardware_key: String,
    pub sysfs_anchor: String,
    /// Proposed line ID for UIM slot 1, not a verified existing line identity.
    pub line_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// `busnum:devnum`; changes whenever the same port re-enumerates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb_generation: Option<String>,
    pub family_hint: &'static str,
    pub qmi_controls: Vec<String>,
    pub mbim_controls: Vec<String>,
    pub serial_ports: Vec<DiscoveredPort>,
    pub net_interfaces: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_port_hint: Option<String>,
    pub issues: Vec<&'static str>,
    /// Incomplete device entry: no guessed AT port or bearer endpoints.
    /// The operator must confirm hardware_key and uim_slot before activation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<NativeDeviceConfig>,
}

/// Walk `sys_root` (normally `/sys`) and `dev_root` (normally `/dev`).
pub fn discover(sys_root: &Path, dev_root: &Path) -> Vec<DiscoveredModem> {
    let sys_canonical = fs::canonicalize(sys_root).unwrap_or_else(|_| sys_root.to_path_buf());
    let mut modems: BTreeMap<String, DiscoveredModem> = BTreeMap::new();
    for modem in discover_usb(sys_root, &sys_canonical, dev_root)
        .into_iter()
        .chain(discover_wwan(sys_root, &sys_canonical))
    {
        match modems.get_mut(&modem.sysfs_anchor) {
            // A module visible both as USB and in the WWAN class is one modem.
            Some(existing) => merge(existing, modem),
            None => {
                modems.insert(modem.sysfs_anchor.clone(), modem);
            }
        }
    }
    let mut result: Vec<_> = modems.into_values().collect();
    for modem in &mut result {
        // Netdevs may be siblings of control channels (MHI/BAM-DMUX), not
        // immediate children of the physical device. Only report host-visible
        // interfaces with an exact sysfs ancestry match; never select one.
        for name in child_names(&sys_root.join("class/net")) {
            if let Ok(device) =
                fs::canonicalize(sys_root.join("class/net").join(&name).join("device"))
            {
                if sysfs_form(&device, &sys_canonical)
                    .is_some_and(|path| Path::new(&path).starts_with(&modem.sysfs_anchor))
                {
                    modem.net_interfaces.push(name);
                }
            }
        }
        finalize(modem);
    }
    result
}

/// Map a canonical path under the (possibly test) sysfs root to its `/sys/...` form.
fn sysfs_form(path: &Path, sys_canonical: &Path) -> Option<String> {
    let relative = path.strip_prefix(sys_canonical).ok()?;
    Some(format!("/sys/{}", relative.to_string_lossy()))
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn driver_name(interface: &Path) -> String {
    fs::read_link(interface.join("driver"))
        .ok()
        .and_then(|target| target.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

fn child_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// `/dev/serial/by-path/<alias>` -> device name it points at (`ttyUSB2`).
fn serial_aliases(dev_root: &Path) -> BTreeMap<String, String> {
    let dir = dev_root.join("serial").join("by-path");
    let mut aliases = BTreeMap::new();
    for alias in child_names(&dir) {
        let Ok(target) = fs::canonicalize(dir.join(&alias)) else {
            continue;
        };
        if let Some(name) = target.file_name() {
            if fs::canonicalize(dev_root.join(name)).ok().as_ref() != Some(&target) {
                continue;
            }
            aliases.insert(
                name.to_string_lossy().into_owned(),
                format!("/dev/serial/by-path/{alias}"),
            );
        }
    }
    aliases
}

/// Quectel's USB serial layout (EC2x/EG2x and the DJI module built on it).
fn quectel_role(interface_number: Option<u8>) -> &'static str {
    match interface_number {
        Some(0) => "diagnostic",
        Some(1) => "nmea",
        Some(2) => "at",
        Some(3) => "modem",
        _ => "unknown",
    }
}

#[derive(Default)]
struct UsbScan {
    qmi_controls: Vec<String>,
    mbim_controls: Vec<String>,
    serial_ports: Vec<DiscoveredPort>,
    net_interfaces: Vec<String>,
    qmi_bound_without_control: bool,
    mbim_bound_without_control: bool,
    unbound_interfaces: usize,
    modem_driver_bound: bool,
}

fn discover_usb(sys_root: &Path, sys_canonical: &Path, dev_root: &Path) -> Vec<DiscoveredModem> {
    let aliases = serial_aliases(dev_root);
    let mut devices: BTreeMap<PathBuf, UsbScan> = BTreeMap::new();
    for name in child_names(&sys_root.join("bus").join("usb").join("devices")) {
        // Interfaces are named `<device>:<config>.<interface>`.
        if !name.contains(':') {
            continue;
        }
        let entry = sys_root.join("bus").join("usb").join("devices").join(&name);
        let Ok(interface) = fs::canonicalize(&entry) else {
            continue;
        };
        let Some(device) = interface.parent().map(Path::to_path_buf) else {
            continue;
        };
        let scan = devices.entry(device).or_default();
        let driver = driver_name(&interface);
        let interface_number = read_trimmed(&interface.join("bInterfaceNumber"))
            .and_then(|value| u8::from_str_radix(&value, 16).ok());
        let controls: Vec<_> = child_names(&interface.join("usbmisc"))
            .into_iter()
            .filter(|name| name.strip_prefix("cdc-wdm").is_some_and(decimal_index))
            .collect();
        match driver.as_str() {
            "qmi_wwan" => {
                scan.modem_driver_bound = true;
                scan.qmi_bound_without_control |= controls.is_empty();
                scan.qmi_controls
                    .extend(controls.iter().map(|c| format!("/dev/{c}")));
            }
            "cdc_mbim" => {
                scan.modem_driver_bound = true;
                scan.mbim_bound_without_control |= controls.is_empty();
                scan.mbim_controls
                    .extend(controls.iter().map(|c| format!("/dev/{c}")));
            }
            "option" | "qcserial" => scan.modem_driver_bound = true,
            "" => scan.unbound_interfaces += 1,
            _ => {}
        }
        let ttys = child_names(&interface)
            .into_iter()
            .filter(|n| n.starts_with("ttyUSB"))
            .chain(
                child_names(&interface.join("tty"))
                    .into_iter()
                    .filter(|n| n.starts_with("ttyACM")),
            );
        for tty in ttys {
            scan.serial_ports.push(DiscoveredPort {
                device: format!("/dev/{tty}"),
                stable_path: aliases.get(&tty).cloned(),
                interface_number,
                driver: driver.clone(),
                role_hint: "unknown",
            });
        }
        scan.net_interfaces
            .extend(child_names(&interface.join("net")));
    }

    let mut modems = Vec::new();
    for (device, scan) in devices {
        let vendor = read_trimmed(&device.join("idVendor")).map(|v| v.to_ascii_lowercase());
        let product_id = read_trimmed(&device.join("idProduct")).map(|v| v.to_ascii_lowercase());
        let dji =
            vendor.as_deref() == Some(DJI_VENDOR) && product_id.as_deref() == Some(DJI_4G_PRODUCT);
        let quectel = vendor.as_deref() == Some(QUECTEL_VENDOR);
        let serial_modem = scan
            .serial_ports
            .iter()
            .any(|p| p.driver == "option" || p.driver == "qcserial");
        // cdc_acm alone may be an Arduino or other non-modem; accept it only
        // when this composite device also has modem evidence or a known ID.
        if !(scan.modem_driver_bound || serial_modem || dji || quectel) {
            continue;
        }
        let Some(anchor) = sysfs_form(&device, sys_canonical) else {
            continue;
        };
        let family_hint = if dji {
            "dji_4g_module"
        } else if quectel {
            "quectel"
        } else {
            "generic"
        };
        let mut serial_ports = scan.serial_ports;
        if dji || quectel {
            for port in &mut serial_ports {
                port.role_hint = quectel_role(port.interface_number);
            }
        }
        let mut issues = Vec::new();
        if scan.qmi_bound_without_control {
            issues.push("qmi_control_missing");
        }
        if scan.mbim_bound_without_control {
            issues.push("mbim_control_missing");
        }
        if scan.unbound_interfaces > 0 {
            issues.push("driver_unbound");
        }
        let manufacturer = read_trimmed(&device.join("manufacturer"));
        let product = read_trimmed(&device.join("product"));
        modems.push(DiscoveredModem {
            bus: "usb",
            hardware_key: String::new(),
            sysfs_anchor: anchor,
            line_id: String::new(),
            usb_id: vendor
                .as_ref()
                .zip(product_id.as_ref())
                .map(|(v, p)| format!("{v}:{p}")),
            // Classic EC2x firmware reports "Android"; that is a placeholder,
            // not the module name.
            product: match (manufacturer, product) {
                (Some(m), Some(p)) if !(m == "Android" && p == "Android") => {
                    Some(format!("{m} {p}"))
                }
                (None, Some(p)) if p != "Android" => Some(p),
                _ => None,
            },
            usb_generation: read_trimmed(&device.join("busnum"))
                .zip(read_trimmed(&device.join("devnum")))
                .map(|(bus, dev)| format!("{bus}:{dev}")),
            family_hint,
            qmi_controls: scan.qmi_controls,
            mbim_controls: scan.mbim_controls,
            serial_ports,
            net_interfaces: scan.net_interfaces,
            at_port_hint: None,
            issues,
            candidate: None,
        });
    }
    modems
}

fn decimal_index(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn wwan_port_kind(name: &str) -> Option<&str> {
    let suffix = name.strip_prefix("wwan")?;
    let index_len = suffix.bytes().take_while(|b| b.is_ascii_digit()).count();
    if index_len == 0 {
        return None;
    }
    let suffix = &suffix[index_len..];
    ["qmi", "mbim", "at"]
        .into_iter()
        .find(|kind| suffix.strip_prefix(*kind).is_some_and(decimal_index))
}

fn wwan_physical_device(parent: &Path) -> PathBuf {
    // MHI exposes each channel below a different child of the PCI function;
    // USB WWAN nodes live below interfaces. Group those siblings by physical
    // USB device / PCI function, not by the individual channel.
    for ancestor in parent.ancestors() {
        if ancestor.join("idVendor").is_file() && ancestor.join("idProduct").is_file() {
            return ancestor.to_path_buf();
        }
        if fs::read_link(ancestor.join("subsystem"))
            .ok()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "pci"))
        {
            return ancestor.to_path_buf();
        }
    }
    // SoC WWAN controllers already sit below their physical remoteproc.
    parent.to_path_buf()
}

/// WWAN class ports (`/dev/wwan<N><kind><M>`); names can change after reboot,
/// so they are discovery observations, not persistent identity anchors.
fn discover_wwan(sys_root: &Path, sys_canonical: &Path) -> Vec<DiscoveredModem> {
    let class = sys_root.join("class").join("wwan");
    let mut modems: BTreeMap<PathBuf, DiscoveredModem> = BTreeMap::new();
    for name in child_names(&class) {
        let Some(kind) = wwan_port_kind(&name) else {
            continue;
        };
        let Ok(port) = fs::canonicalize(class.join(&name)) else {
            continue;
        };
        // <physical>/wwan/wwan<N>/<port>
        let Some(physical) = port
            .parent()
            .and_then(Path::parent)
            .filter(|dir| dir.file_name().is_some_and(|n| n == "wwan"))
            .and_then(Path::parent)
            .map(Path::to_path_buf)
        else {
            continue;
        };
        let physical = wwan_physical_device(&physical);
        let Some(anchor) = sysfs_form(&physical, sys_canonical) else {
            continue;
        };
        let modem = modems
            .entry(physical.clone())
            .or_insert_with(|| DiscoveredModem {
                bus: "wwan",
                hardware_key: String::new(),
                sysfs_anchor: anchor,
                line_id: String::new(),
                usb_id: None,
                product: None,
                usb_generation: None,
                family_hint: "wwan_class",
                qmi_controls: Vec::new(),
                mbim_controls: Vec::new(),
                serial_ports: Vec::new(),
                net_interfaces: child_names(&physical.join("net")),
                at_port_hint: None,
                issues: Vec::new(),
                candidate: None,
            });
        let device = format!("/dev/{name}");
        match kind {
            "qmi" => modem.qmi_controls.push(device),
            "mbim" => modem.mbim_controls.push(device),
            "at" => modem.serial_ports.push(DiscoveredPort {
                device,
                stable_path: None,
                interface_number: None,
                driver: "wwan".into(),
                role_hint: "at",
            }),
            _ => {}
        }
    }
    modems.into_values().collect()
}

fn merge(existing: &mut DiscoveredModem, other: DiscoveredModem) {
    for control in other.qmi_controls {
        if !existing.qmi_controls.contains(&control) {
            existing.qmi_controls.push(control);
        }
    }
    for control in other.mbim_controls {
        if !existing.mbim_controls.contains(&control) {
            existing.mbim_controls.push(control);
        }
    }
    for port in other.serial_ports {
        if !existing
            .serial_ports
            .iter()
            .any(|p| p.device == port.device)
        {
            existing.serial_ports.push(port);
        }
    }
    for interface in other.net_interfaces {
        if !existing.net_interfaces.contains(&interface) {
            existing.net_interfaces.push(interface);
        }
    }
    existing.issues.extend(other.issues);
}

fn finalize(modem: &mut DiscoveredModem) {
    modem.hardware_key =
        crate::hardware::cellular::modem_manager::physical_device_slot_id(&modem.sysfs_anchor)
            .unwrap_or_else(|| format!("sysfs:{}", modem.sysfs_anchor.trim_start_matches("/sys/")));
    modem.line_id = crate::hardware::cellular::bindings::physical_line_id(&modem.hardware_key, 1);
    modem.serial_ports.sort_by(|a, b| {
        a.interface_number
            .cmp(&b.interface_number)
            .then_with(|| a.device.cmp(&b.device))
    });
    let at_ports: Vec<_> = modem
        .serial_ports
        .iter()
        .filter(|p| p.role_hint == "at")
        .collect();
    modem.at_port_hint = match at_ports.as_slice() {
        [port] => Some(port.open_path().to_string()),
        _ => None,
    };
    if !modem.serial_ports.is_empty() {
        // Even a conventional port role is unverified until an operator
        // probes it in an exclusive-owner maintenance window.
        modem.issues.push("at_port_requires_probe");
        if at_ports.len() > 1 {
            modem.issues.push("multiple_at_ports");
        }
    } else {
        modem.issues.push("at_port_missing");
    }
    for controls in [
        &mut modem.qmi_controls,
        &mut modem.mbim_controls,
        &mut modem.net_interfaces,
    ] {
        controls.sort();
        controls.dedup();
    }
    if modem.qmi_controls.len() > 1 {
        modem.issues.push("multiple_qmi_controls");
    }
    if modem.mbim_controls.len() > 1 {
        modem.issues.push("multiple_mbim_controls");
    }
    if !modem.qmi_controls.is_empty() && !modem.mbim_controls.is_empty() {
        modem.issues.push("multiple_control_protocols");
    }
    modem.issues.push("hardware_key_requires_confirmation");
    modem.issues.push("uim_slot_requires_confirmation");
    modem.candidate = candidate(modem);
    if modem.qmi_controls.is_empty()
        && modem.mbim_controls.is_empty()
        && modem.serial_ports.is_empty()
    {
        modem.issues.push("no_control_interface");
    }
    modem.issues.sort_unstable();
    modem.issues.dedup();
}

fn candidate(modem: &DiscoveredModem) -> Option<NativeDeviceConfig> {
    let (protocol, control_device) = match (
        modem.qmi_controls.as_slice(),
        modem.mbim_controls.as_slice(),
    ) {
        ([qmi], []) => (NativeProtocol::Qmi, qmi.clone()),
        ([], [mbim]) => (NativeProtocol::Mbim, mbim.clone()),
        // Never select the first of multiple control ports, or turn an AT
        // layout hint into an enabled command endpoint.
        _ => return None,
    };
    let config = NativeDeviceConfig {
        hardware_key: modem.hardware_key.clone(),
        sysfs_anchor: modem.sysfs_anchor.clone(),
        protocol,
        control_device,
        at_device: None,
        // Receiving stored SMS deletes it from the modem; never implied.
        sms_reception_enabled: false,
        uim_slot: 1,
        // Bearer endpoints must come from device evidence, never guessed.
        ims: None,
        data: None,
    };
    config.validate().ok().map(|()| config)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(label: &str) -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "simadmin-discovery-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("sys/bus/usb/devices")).unwrap();
            fs::create_dir_all(root.join("sys/bus/usb/drivers")).unwrap();
            fs::create_dir_all(root.join("sys/class/wwan")).unwrap();
            fs::create_dir_all(root.join("dev/serial/by-path")).unwrap();
            Self { root }
        }
        fn sys(&self) -> PathBuf {
            self.root.join("sys")
        }
        fn dev(&self) -> PathBuf {
            self.root.join("dev")
        }
        fn write(&self, path: &Path, value: &str) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{value}\n")).unwrap();
        }
        /// A USB device under /sys/devices/.../<port> with the given id.
        fn usb_device(&self, port: &str, vendor: &str, product: &str) -> PathBuf {
            let device = self.sys().join("devices/platform/usb/usb1").join(port);
            self.write(&device.join("idVendor"), vendor);
            self.write(&device.join("idProduct"), product);
            self.write(&device.join("busnum"), "1");
            self.write(&device.join("devnum"), "4");
            symlink(&device, self.sys().join("bus/usb/devices").join(port)).unwrap();
            device
        }
        /// One interface; `driver` empty means unbound.
        fn interface(&self, device: &Path, number: u8, driver: &str, children: &[&str]) -> PathBuf {
            let port = device.file_name().unwrap().to_string_lossy().into_owned();
            let name = format!("{port}:1.{number}");
            let interface = device.join(&name);
            self.write(
                &interface.join("bInterfaceNumber"),
                &format!("{number:02x}"),
            );
            if !driver.is_empty() {
                let driver_dir = self.sys().join("bus/usb/drivers").join(driver);
                fs::create_dir_all(&driver_dir).unwrap();
                symlink(&driver_dir, interface.join("driver")).unwrap();
            }
            for child in children {
                fs::create_dir_all(interface.join(child)).unwrap();
            }
            symlink(&interface, self.sys().join("bus/usb/devices").join(name)).unwrap();
            interface
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn quectel_qmi_composition_keeps_the_at_port_as_an_unverified_hint() {
        let tree = Tree::new("quectel-qmi");
        let device = tree.usb_device("1-1", "2c7c", "0125");
        tree.interface(&device, 0, "option", &["ttyUSB0"]);
        tree.interface(&device, 1, "option", &["ttyUSB1"]);
        tree.interface(&device, 2, "option", &["ttyUSB2"]);
        tree.interface(&device, 3, "option", &["ttyUSB3"]);
        tree.interface(&device, 4, "qmi_wwan", &["usbmisc/cdc-wdm0", "net/wwan0"]);
        tree.write(&tree.dev().join("ttyUSB2"), "");
        symlink(
            "../../ttyUSB2",
            tree.dev().join("serial/by-path/platform-usb-1:1.2-port0"),
        )
        .unwrap();

        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        let modem = &modems[0];
        assert_eq!(modem.family_hint, "quectel");
        assert_eq!(modem.sysfs_anchor, "/sys/devices/platform/usb/usb1/1-1");
        assert_eq!(modem.hardware_key, "sysfs:devices/platform/usb/usb1/1-1");
        assert_eq!(modem.qmi_controls, vec!["/dev/cdc-wdm0"]);
        assert_eq!(modem.net_interfaces, vec!["wwan0"]);
        assert_eq!(
            modem.at_port_hint.as_deref(),
            Some("/dev/serial/by-path/platform-usb-1:1.2-port0")
        );
        assert!(modem.issues.contains(&"at_port_requires_probe"));
        assert!(modem.issues.contains(&"hardware_key_requires_confirmation"));
        let candidate = modem.candidate.as_ref().expect("candidate");
        assert_eq!(candidate.protocol, NativeProtocol::Qmi);
        assert_eq!(candidate.control_device, "/dev/cdc-wdm0");
        assert!(candidate.at_device.is_none());
        assert!(candidate.ims.is_none() && candidate.data.is_none());
        assert!(!candidate.sms_reception_enabled);
        assert_eq!(candidate.line_id(), modem.line_id);
    }

    #[test]
    fn serial_only_quectel_reports_an_at_hint_without_selecting_it() {
        let tree = Tree::new("quectel-ecm");
        let device = tree.usb_device("1-2", "2c7c", "6005");
        tree.interface(&device, 2, "option", &["ttyUSB5"]);
        tree.interface(&device, 4, "cdc_ether", &["net/usb0"]);
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        assert!(modems[0].candidate.is_none());
        assert_eq!(modems[0].at_port_hint.as_deref(), Some("/dev/ttyUSB5"));
        assert!(modems[0].issues.contains(&"at_port_requires_probe"));
    }

    #[test]
    fn unbound_dji_module_is_reported_instead_of_dropped() {
        let tree = Tree::new("dji");
        let device = tree.usb_device("1-3", "2ca3", "4006");
        for number in 0..=4 {
            tree.interface(&device, number, "", &[]);
        }
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        let modem = &modems[0];
        assert_eq!(modem.family_hint, "dji_4g_module");
        assert!(modem.issues.contains(&"driver_unbound"));
        assert!(modem.issues.contains(&"at_port_missing"));
        assert!(modem.issues.contains(&"no_control_interface"));
        assert!(modem.candidate.is_none());
    }

    #[test]
    fn unrelated_usb_devices_are_ignored() {
        let tree = Tree::new("unrelated");
        let keyboard = tree.usb_device("1-4", "046d", "c31c");
        tree.interface(&keyboard, 0, "usbhid", &[]);
        let arduino = tree.usb_device("1-5", "2341", "0043");
        tree.interface(&arduino, 0, "cdc_acm", &["tty/ttyACM0"]);
        assert!(discover(&tree.sys(), &tree.dev()).is_empty());
    }

    #[test]
    fn wwan_class_ports_group_under_their_physical_device() {
        let tree = Tree::new("wwan");
        let physical = tree.sys().join("devices/platform/soc/modem");
        for port in ["wwan0qmi0", "wwan0at0", "wwan0at1"] {
            let dir = physical.join("wwan/wwan0").join(port);
            fs::create_dir_all(&dir).unwrap();
            symlink(&dir, tree.sys().join("class/wwan").join(port)).unwrap();
        }
        let device_dir = physical.join("wwan/wwan0");
        symlink(&device_dir, tree.sys().join("class/wwan/wwan0")).unwrap();
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        let modem = &modems[0];
        assert_eq!(modem.bus, "wwan");
        assert_eq!(modem.sysfs_anchor, "/sys/devices/platform/soc/modem");
        assert_eq!(modem.qmi_controls, vec!["/dev/wwan0qmi0"]);
        assert!(modem.at_port_hint.is_none());
        assert!(modem.issues.contains(&"multiple_at_ports"));
        let candidate = modem.candidate.as_ref().expect("candidate");
        assert_eq!(candidate.protocol, NativeProtocol::Qmi);
        assert!(candidate.at_device.is_none());
    }

    #[test]
    fn mbim_with_acm_is_vendor_neutral_but_missing_controls_are_reported() {
        let tree = Tree::new("mbim");
        let device = tree.usb_device("1-6", "1199", "9071");
        tree.interface(&device, 0, "cdc_mbim", &["usbmisc/cdc-wdm1", "net/wwan0"]);
        tree.interface(&device, 2, "cdc_acm", &["tty/ttyACM0"]);
        let modems = discover(&tree.sys(), &tree.dev());
        let modem = &modems[0];
        assert_eq!(modem.mbim_controls, vec!["/dev/cdc-wdm1"]);
        assert_eq!(modem.serial_ports[0].device, "/dev/ttyACM0");
        assert_eq!(
            modem.candidate.as_ref().unwrap().protocol,
            NativeProtocol::Mbim
        );
        assert!(modem.at_port_hint.is_none());
        let broken = tree.usb_device("1-7", "1199", "9071");
        tree.interface(&broken, 0, "cdc_mbim", &[]);
        let modems = discover(&tree.sys(), &tree.dev());
        assert!(modems[1].issues.contains(&"mbim_control_missing"));
        assert!(modems[1].candidate.is_none());
    }

    #[test]
    fn ecm_only_quectel_and_missing_qmi_are_not_silently_dropped() {
        let tree = Tree::new("missing");
        let device = tree.usb_device("1-1", "2c7c", "0125");
        tree.interface(&device, 4, "cdc_ether", &["net/usb0"]);
        let broken = tree.usb_device("1-2", "1234", "1234");
        tree.interface(&broken, 4, "qmi_wwan", &[]);
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 2);
        assert_eq!(modems[0].net_interfaces, vec!["usb0"]);
        assert!(modems[0].issues.contains(&"no_control_interface"));
        assert!(modems[1].issues.contains(&"qmi_control_missing"));
        assert!(modems.iter().all(|m| m.candidate.is_none()));
    }

    #[test]
    fn ambiguous_control_ports_or_protocols_never_pick_the_first() {
        for mixed in [false, true] {
            let tree = Tree::new("ambiguous");
            let device = tree.usb_device("1-1", "2c7c", "0125");
            tree.interface(&device, 4, "qmi_wwan", &["usbmisc/cdc-wdm0"]);
            tree.interface(
                &device,
                5,
                if mixed { "cdc_mbim" } else { "qmi_wwan" },
                &["usbmisc/cdc-wdm1"],
            );
            let modems = discover(&tree.sys(), &tree.dev());
            assert!(modems[0].candidate.is_none());
            assert!(modems[0].issues.contains(&if mixed {
                "multiple_control_protocols"
            } else {
                "multiple_qmi_controls"
            }));
            assert!(!modems[0].issues.contains(&"no_control_interface"));
        }
    }

    #[test]
    fn mhi_sibling_channels_group_at_the_pci_function_and_find_sibling_netdevs() {
        let tree = Tree::new("mhi");
        let physical = tree
            .sys()
            .join("devices/pci0000:00/0000:00:01.0/0000:01:00.0");
        fs::create_dir_all(&physical).unwrap();
        symlink("/sys/bus/pci", physical.join("subsystem")).unwrap();
        for (channel, port) in [("mhi0_QMI", "wwan0qmi0"), ("mhi0_DUN", "wwan0at0")] {
            let dir = physical.join(channel).join("wwan/wwan0").join(port);
            fs::create_dir_all(&dir).unwrap();
            symlink(&dir, tree.sys().join("class/wwan").join(port)).unwrap();
        }
        let net = physical.join("mhi0_IP_HW0");
        fs::create_dir_all(&net).unwrap();
        fs::create_dir_all(tree.sys().join("class/net/wwan0")).unwrap();
        symlink(&net, tree.sys().join("class/net/wwan0/device")).unwrap();
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        assert_eq!(
            modems[0].sysfs_anchor,
            "/sys/devices/pci0000:00/0000:00:01.0/0000:01:00.0"
        );
        assert_eq!(modems[0].net_interfaces, vec!["wwan0"]);
        assert_eq!(modems[0].at_port_hint.as_deref(), Some("/dev/wwan0at0"));
        assert!(modems[0].candidate.as_ref().unwrap().at_device.is_none());
    }

    #[test]
    fn usb_and_wwan_views_merge_and_unknown_wwan_ports_are_ignored() {
        let tree = Tree::new("usb-wwan");
        let device = tree.usb_device("1-1", "2c7c", "0125");
        let interface = tree.interface(&device, 4, "qmi_wwan", &[]);
        for port in [
            "wwan0qmi0",
            "wwan0qcdm0",
            "wwan0qmi",
            "wwanqmi0",
            "notwwan0qmi0",
        ] {
            let dir = interface.join("wwan/wwan0").join(port);
            fs::create_dir_all(&dir).unwrap();
            symlink(&dir, tree.sys().join("class/wwan").join(port)).unwrap();
        }
        let modems = discover(&tree.sys(), &tree.dev());
        assert_eq!(modems.len(), 1);
        assert_eq!(modems[0].qmi_controls, vec!["/dev/wwan0qmi0"]);
        assert_eq!(modems[0].sysfs_anchor, "/sys/devices/platform/usb/usb1/1-1");
        assert!(modems[0].candidate.is_some());
    }

    #[test]
    fn serial_aliases_must_resolve_to_the_reported_device() {
        let tree = Tree::new("alias");
        let device = tree.usb_device("1-1", "2c7c", "0125");
        tree.interface(&device, 2, "option", &["ttyUSB2"]);
        let other = tree.root.join("outside/ttyUSB2");
        tree.write(&other, "");
        tree.write(&tree.dev().join("ttyUSB2"), "");
        symlink(&other, tree.dev().join("serial/by-path/wrong-device")).unwrap();
        symlink("../../missing", tree.dev().join("serial/by-path/broken")).unwrap();
        let modems = discover(&tree.sys(), &tree.dev());
        assert!(modems[0].serial_ports[0].stable_path.is_none());
        assert_eq!(modems[0].at_port_hint.as_deref(), Some("/dev/ttyUSB2"));
    }

    #[test]
    fn empty_roots_discover_nothing() {
        let tree = Tree::new("empty");
        assert!(discover(&tree.sys(), &tree.dev()).is_empty());
        assert!(discover(Path::new("/nonexistent-sys"), Path::new("/nonexistent-dev")).is_empty());
    }
}
