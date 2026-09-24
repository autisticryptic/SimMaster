//! Opt-in maintenance for the first-generation DJI USB modem (2ca3:4006).
//! Discovery never calls this module. No service stop, NV/USB-ID rewrite or
//! rollback; failures report completed steps for explicit operator recovery.
use crate::hardware::cellular::backends::NativeError;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const VID: &str = "2ca3";
const PID: &str = "4006";

#[derive(Debug, Clone, Serialize)]
pub struct Interface {
    pub number: u8,
    pub name: String,
    pub driver: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct DjiPlan {
    pub usb_device: String,
    pub sysfs_anchor: PathBuf,
    pub generation: String,
    pub interfaces: Vec<Interface>,
    pub requires_confirmation: bool,
    pub effects: Vec<&'static str>,
}
#[derive(Debug, Serialize)]
pub struct DjiResult {
    pub status: &'static str,
    pub completed_steps: Vec<String>,
    pub failed_step: Option<String>,
    pub qmi_ready: bool,
}
fn error(reason: &str) -> NativeError {
    NativeError::Protocol(reason.into())
}
fn read(path: &Path) -> Result<String, NativeError> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|_| error("dji_sysfs_unavailable"))
}
fn driver(path: &Path) -> String {
    fs::read_link(path.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}
fn valid_usb_name(name: &str) -> bool {
    let Some((bus, ports)) = name.split_once('-') else {
        return false;
    };
    !bus.is_empty()
        && bus.bytes().all(|b| b.is_ascii_digit())
        && !ports.is_empty()
        && ports
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn plan_at(sys: &Path, name: &str) -> Result<DjiPlan, NativeError> {
    if !valid_usb_name(name) {
        return Err(error("dji_usb_name_invalid"));
    }
    let root = sys.join("bus/usb/devices");
    let anchor = fs::canonicalize(root.join(name)).map_err(|_| error("dji_device_absent"))?;
    let sys = fs::canonicalize(sys).map_err(|_| error("dji_sysfs_unavailable"))?;
    if !anchor.starts_with(sys.join("devices")) {
        return Err(error("dji_physical_anchor_invalid"));
    }
    if read(&anchor.join("idVendor"))?.to_lowercase() != VID
        || read(&anchor.join("idProduct"))?.to_lowercase() != PID
    {
        return Err(error("dji_usb_identity_mismatch"));
    }
    let mut count = 0;
    for entry in fs::read_dir(&root).map_err(|_| error("dji_inventory_unavailable"))? {
        let entry = entry.map_err(|_| error("dji_inventory_unavailable"))?;
        if !valid_usb_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        if read(&entry.path().join("idVendor")).ok().as_deref() == Some(VID)
            && read(&entry.path().join("idProduct")).ok().as_deref() == Some(PID)
        {
            count += 1;
        }
    }
    // new_id is driver-wide, not per physical device. Never pretend it is
    // scoped when another modem with the same identity is connected.
    if count != 1 {
        return Err(error("dji_requires_single_matching_usb_device"));
    }
    let bus = read(&anchor.join("busnum"))?
        .parse::<u16>()
        .map_err(|_| error("dji_generation_invalid"))?;
    let dev = read(&anchor.join("devnum"))?
        .parse::<u16>()
        .map_err(|_| error("dji_generation_invalid"))?;
    if bus == 0 || dev == 0 || bus > 999 || dev > 127 {
        return Err(error("dji_generation_invalid"));
    }
    let configuration = read(&anchor.join("bConfigurationValue"))?
        .parse::<u8>()
        .map_err(|_| error("dji_configuration_invalid"))?;
    let mut interfaces = Vec::new();
    for entry in fs::read_dir(&anchor).map_err(|_| error("dji_interfaces_unavailable"))? {
        let entry = entry.map_err(|_| error("dji_interfaces_unavailable"))?;
        let iface = entry.file_name().to_string_lossy().into_owned();
        if !iface.starts_with(&format!("{name}:{configuration}.")) {
            continue;
        }
        let number = u8::from_str_radix(&read(&entry.path().join("bInterfaceNumber"))?, 16)
            .map_err(|_| error("dji_interface_invalid"))?;
        if iface != format!("{name}:{configuration}.{number}") || number > 4 {
            return Err(error("dji_composition_unsupported"));
        }
        let current = driver(&entry.path());
        if !matches!(current.as_str(), "" | "option" | "qmi_wwan") {
            return Err(error("dji_foreign_interface_driver"));
        }
        interfaces.push(Interface {
            number,
            name: iface,
            driver: current,
        });
    }
    interfaces.sort_by_key(|i| i.number);
    if interfaces.iter().map(|i| i.number).collect::<Vec<_>>() != [0, 1, 2, 3, 4] {
        return Err(error("dji_composition_unsupported"));
    }
    Ok(DjiPlan {
        usb_device: name.into(),
        sysfs_anchor: anchor,
        generation: format!("{bus}:{dev}"),
        interfaces,
        requires_confirmation: true,
        effects: vec![
            "bind qmi_wwan to interface 4",
            "bind option serial interfaces 0..3",
            "assert USB CDC DTR on interface 4",
            "driver-wide dynamic ID registration until module unload",
            "read QMI DMS operating mode; do not change it",
        ],
    })
}

pub fn plan(name: &str) -> Result<DjiPlan, NativeError> {
    plan_at(Path::new("/sys"), name)
}
fn unchanged(plan: &DjiPlan) -> Result<(), NativeError> {
    let fresh = self::plan(&plan.usb_device)?;
    if fresh.generation != plan.generation || fresh.sysfs_anchor != plan.sysfs_anchor {
        return Err(error("dji_usb_generation_changed"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn assert_dtr(plan: &DjiPlan) -> Result<(), NativeError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
    #[repr(C)]
    struct Control {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        timeout: u32,
        data: *mut libc::c_void,
    }
    let (bus, device) = plan
        .generation
        .split_once(':')
        .ok_or_else(|| error("dji_generation_invalid"))?;
    let bus: u16 = bus.parse().map_err(|_| error("dji_generation_invalid"))?;
    let device: u16 = device
        .parse()
        .map_err(|_| error("dji_generation_invalid"))?;
    unchanged(plan)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(format!("/dev/bus/usb/{bus:03}/{device:03}"))
        .map_err(|_| error("dji_usbfs_open_failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| error("dji_usbfs_open_failed"))?;
    if !metadata.file_type().is_char_device()
        || libc::major(metadata.rdev()) != 189
        || libc::minor(metadata.rdev()) != (u32::from(bus) - 1) * 128 + u32::from(device) - 1
    {
        return Err(error("dji_usbfs_device_identity_mismatch"));
    }
    // Linux USBDEVFS_CONTROL, sized for the target architecture's pointer.
    let request = ((3u32 << 30)
        | ((std::mem::size_of::<Control>() as u32) << 16)
        | ((b'U' as u32) << 8)) as libc::c_ulong;
    for value in [0, 1] {
        let mut control = Control {
            request_type: 0x21,
            request: 0x22,
            value,
            index: 4,
            length: 0,
            timeout: 3000,
            data: std::ptr::null_mut(),
        };
        if unsafe { libc::ioctl(file.as_raw_fd(), request, &mut control) } < 0 {
            return Err(error("dji_dtr_control_failed"));
        }
    }
    unchanged(plan)
}

#[cfg(target_os = "linux")]
fn write_step(plan: &DjiPlan, path: &Path, value: &str) -> Result<(), NativeError> {
    unchanged(plan)?;
    fs::write(path, value).map_err(|_| error("dji_driver_write_failed"))?;
    unchanged(plan)
}

#[cfg(target_os = "linux")]
async fn manager_absent(connection: &zbus::Connection) -> Result<(), NativeError> {
    let bus = zbus::fdo::DBusProxy::new(connection)
        .await
        .map_err(|_| error("dji_owner_check_unavailable"))?;
    if bus
        .name_has_owner(
            "org.freedesktop.ModemManager1"
                .try_into()
                .expect("DBus name"),
        )
        .await
        .map_err(|_| error("dji_owner_check_unavailable"))?
    {
        return Err(error("dji_modemmanager_must_be_stopped_by_operator"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn apply(
    name: String,
    expected_generation: String,
    confirm_usb_device: String,
) -> Result<DjiResult, NativeError> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    if name != confirm_usb_device {
        return Err(error("dji_explicit_confirmation_required"));
    }
    let plan = plan(&name)?;
    if plan.generation != expected_generation {
        return Err(error("dji_plan_stale"));
    }
    let connection = Arc::new(
        zbus::Connection::system()
            .await
            .map_err(|_| error("dji_owner_check_unavailable"))?,
    );
    manager_absent(&connection).await?;
    crate::hardware::cellular::backends::ensure_mm_handover_clear()
        .map_err(|_| error("dji_existing_native_owner_or_receipt"))?;
    let directory = Path::new("/run/simadmin/native-control");
    fs::create_dir_all(directory).map_err(|_| error("dji_lock_directory_failed"))?;
    let lock_path = directory.join(format!(
        "physical-{:x}",
        md5::compute(plan.sysfs_anchor.as_os_str().as_encoded_bytes())
    ));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(lock_path)
        .map_err(|_| error("dji_physical_lock_failed"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(error("dji_physical_device_owned"));
    }
    // An attached network link may be in use even without MM. Require this
    // special repair to begin with interface 4 unbound, not a live data path.
    if !plan.interfaces[4].driver.is_empty() {
        return Err(error("dji_qmi_interface_must_be_unbound"));
    }
    if plan.interfaces[..4]
        .iter()
        .any(|iface| iface.driver == "qmi_wwan")
    {
        return Err(error("dji_preexisting_serial_driver_requires_review"));
    }
    let usb_drivers = Path::new("/sys/bus/usb/drivers");
    let option_ids = Path::new("/sys/bus/usb-serial/drivers/option1/new_id");
    for path in [
        usb_drivers.join("qmi_wwan/new_id"),
        usb_drivers.join("qmi_wwan/bind"),
        usb_drivers.join("option/bind"),
        option_ids.to_path_buf(),
    ] {
        if !path.is_file() {
            return Err(error("dji_load_required_drivers_before_apply"));
        }
    }
    manager_absent(&connection).await?;
    let marker = directory.join(format!(
        "session-dji-{}-maintenance.json",
        name.replace('.', "-")
    ));
    let mut marker_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&marker)
        .map_err(|_| error("dji_maintenance_receipt_exists"))?;
    use std::io::Write;
    marker_file
        .write_all(&serde_json::to_vec(&plan).map_err(|_| error("dji_plan_encoding_failed"))?)
        .and_then(|_| marker_file.sync_all())
        .map_err(|_| error("dji_receipt_write_failed"))?;
    let mut result = DjiResult {
        status: "unconfirmed",
        completed_steps: Vec::new(),
        failed_step: None,
        qmi_ready: false,
    };
    macro_rules! step {
        ($name:expr,$action:expr) => {{
            if manager_absent(&connection).await.is_err() {
                result.failed_step = Some("owner_changed".into());
                return Ok(result);
            }
            if $action.is_err() {
                result.failed_step = Some($name.into());
                return Ok(result);
            }
            result.completed_steps.push($name.into());
        }};
    }
    step!("dtr", assert_dtr(&plan));
    step!(
        "qmi_dynamic_id",
        write_step(&plan, &usb_drivers.join("qmi_wwan/new_id"), "2ca3 4006")
    );
    let qmi = &plan.interfaces[4];
    if driver(&plan.sysfs_anchor.join(&qmi.name)) != "qmi_wwan" {
        step!(
            "qmi_bind",
            write_step(&plan, &usb_drivers.join("qmi_wwan/bind"), &qmi.name)
        );
    }
    if driver(&plan.sysfs_anchor.join(&qmi.name)) != "qmi_wwan" {
        result.failed_step = Some("qmi_binding_readback".into());
        return Ok(result);
    }
    // qmi_wwan new_id may also probe vendor-specific serial interfaces. Undo
    // only claims created from an initially unbound interface in THIS plan.
    for iface in &plan.interfaces[..4] {
        let current = driver(&plan.sysfs_anchor.join(&iface.name));
        if current == "qmi_wwan" && iface.driver.is_empty() {
            step!(
                format!("release_false_qmi_{}", iface.number),
                write_step(&plan, &usb_drivers.join("qmi_wwan/unbind"), &iface.name)
            );
        } else if !matches!(current.as_str(), "" | "option") {
            result.failed_step = Some("preexisting_serial_driver_requires_review".into());
            return Ok(result);
        }
    }
    step!(
        "serial_dynamic_id",
        write_step(&plan, option_ids, "2ca3 4006")
    );
    for iface in &plan.interfaces[..4] {
        if driver(&plan.sysfs_anchor.join(&iface.name)) != "option" {
            step!(
                format!("serial_bind_{}", iface.number),
                write_step(&plan, &usb_drivers.join("option/bind"), &iface.name)
            );
        }
        if driver(&plan.sysfs_anchor.join(&iface.name)) != "option" {
            result.failed_step = Some("serial_binding_readback".into());
            return Ok(result);
        }
    }
    let controls = fs::read_dir(plan.sysfs_anchor.join(&qmi.name).join("usbmisc"))
        .map_err(|_| error("dji_qmi_control_missing"))?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.strip_prefix("cdc-wdm")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect::<Vec<_>>();
    let [control] = controls.as_slice() else {
        result.failed_step = Some("qmi_control_ambiguous_or_missing".into());
        return Ok(result);
    };
    // Fixed read-only DMS action; shared bounded native process runner.
    let request = crate::hardware::cellular::backends::protocol::CommandRequest::query(
        crate::hardware::cellular::backends::config::NativeProtocol::Qmi,
        &format!("/dev/{control}"),
        "--dms-get-operating-mode",
    );
    step!(
        "qmi_dms_probe",
        crate::hardware::cellular::backends::io::run_process(&request)
            .await
            .and_then(|text| {
                match crate::hardware::cellular::backends::protocol::labelled(
                    &text,
                    "Operating mode",
                )
                .as_deref()
                {
                    Some(
                        "online"
                        | "offline"
                        | "low-power"
                        | "persistent-low-power"
                        | "factory-test"
                        | "reset"
                        | "shutting-down",
                    ) => Ok(()),
                    _ => Err(error("dji_dms_mode_unconfirmed")),
                }
            })
    );
    step!("generation_readback", unchanged(&plan));
    fs::remove_file(marker).map_err(|_| error("dji_receipt_cleanup_failed"))?;
    result.status = "bindings_and_dms_verified";
    result.qmi_ready = true;
    Ok(result)
}

#[cfg(not(target_os = "linux"))]
pub async fn apply(
    _name: String,
    _generation: String,
    _confirm: String,
) -> Result<DjiResult, NativeError> {
    Err(NativeError::Unsupported("dji_maintenance_requires_linux"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    struct Tree {
        root: PathBuf,
    }
    impl Tree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "simadmin-dji-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(root.join("bus/usb/devices")).unwrap();
            Self { root }
        }
        fn device(&self, name: &str, vendor: &str) -> PathBuf {
            let dev = self.root.join("devices/usb").join(name);
            fs::create_dir_all(&dev).unwrap();
            for (k, v) in [
                ("idVendor", vendor),
                ("idProduct", "4006"),
                ("busnum", "1"),
                ("devnum", "7"),
                ("bConfigurationValue", "1"),
            ] {
                fs::write(dev.join(k), v).unwrap();
            }
            for i in 0..5 {
                let p = dev.join(format!("{name}:1.{i}"));
                fs::create_dir(&p).unwrap();
                fs::write(p.join("bInterfaceNumber"), format!("{i:02x}")).unwrap();
            }
            symlink(&dev, self.root.join("bus/usb/devices").join(name)).unwrap();
            dev
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn only_exact_usb_port_names_are_accepted() {
        for s in ["../1-1", "1-1:1.4", "1-1/driver", "usb1", "1-"] {
            assert!(!valid_usb_name(s));
        }
        assert!(valid_usb_name("1-2.3"));
    }
    #[test]
    fn passive_plan_reports_exact_identity_and_generation() {
        let t = Tree::new();
        t.device("1-1", VID);
        let p = plan_at(&t.root, "1-1").unwrap();
        assert_eq!(p.generation, "1:7");
        assert_eq!(p.interfaces.len(), 5);
        assert!(p.requires_confirmation);
    }
    #[test]
    fn wrong_id_extra_interface_and_second_matching_modem_are_rejected() {
        let t = Tree::new();
        let dev = t.device("1-1", "1234");
        assert!(plan_at(&t.root, "1-1").is_err());
        fs::write(dev.join("idVendor"), VID).unwrap();
        t.device("1-2", VID);
        assert!(plan_at(&t.root, "1-1").is_err());
    }
    #[test]
    fn foreign_driver_is_not_detached_by_a_repair_plan() {
        let t = Tree::new();
        let dev = t.device("1-1", VID);
        symlink("/sys/bus/usb/drivers/foreign", dev.join("1-1:1.0/driver")).unwrap();
        assert!(plan_at(&t.root, "1-1").is_err());
    }
}
