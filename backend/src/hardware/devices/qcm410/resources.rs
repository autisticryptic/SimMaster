//! Installation of update-package resources required by the QCM410 driver.

use std::{fs, path::PathBuf, process::Command};

const RESOURCE_SUBDIR: &str = "devices/qcm410/system";
const SECONDARY_QMI_SERVICE_NAME: &str = "simadmin-secondary-qmi.service";
const SECONDARY_QMI_SERVICE_PATH: &str = "/etc/systemd/system/simadmin-secondary-qmi.service";
const MODEM_RECOVERY_SERVICE_NAME: &str = "simadmin-modem-recovery.service";
const MODEM_RECOVERY_TIMER_NAME: &str = "simadmin-modem-recovery.timer";
const MODEM_RECOVERY_SCRIPT_PATH: &str = "/usr/local/bin/simadmin-modem-recovery.sh";
const MODEM_RECOVERY_SERVICE_PATH: &str = "/etc/systemd/system/simadmin-modem-recovery.service";
const MODEM_RECOVERY_TIMER_PATH: &str = "/etc/systemd/system/simadmin-modem-recovery.timer";

fn source_path(staging_dir: &str, name: &str) -> PathBuf {
    PathBuf::from(staging_dir).join(RESOURCE_SUBDIR).join(name)
}

/// Install and optionally activate QCM410 boot and recovery resources.
///
/// The generic installer and OTA flow know only the unpacked package root.
/// Unit names, source layout, permissions and ModemManager ordering remain
/// entirely inside this driver.
pub fn install(staging_dir: &str, restart_now: bool) -> String {
    let resources = [
        (
            source_path(staging_dir, SECONDARY_QMI_SERVICE_NAME),
            SECONDARY_QMI_SERVICE_PATH,
            "644",
        ),
        (
            source_path(staging_dir, "simadmin-modem-recovery.sh"),
            MODEM_RECOVERY_SCRIPT_PATH,
            "755",
        ),
        (
            source_path(staging_dir, MODEM_RECOVERY_SERVICE_NAME),
            MODEM_RECOVERY_SERVICE_PATH,
            "644",
        ),
        (
            source_path(staging_dir, MODEM_RECOVERY_TIMER_NAME),
            MODEM_RECOVERY_TIMER_PATH,
            "644",
        ),
    ];

    if resources.iter().any(|(source, _, _)| !source.is_file()) {
        return "QCM410 resources not present, existing setup preserved".to_string();
    }

    for directory in ["/usr/local/bin", "/etc/systemd/system"] {
        if let Err(error) = fs::create_dir_all(directory) {
            return format!("QCM410 resource directory unavailable: {error}");
        }
    }
    for (source, destination, mode) in &resources {
        if let Err(error) = fs::copy(source, destination) {
            return format!("QCM410 resource install failed: {error}");
        }
        let _ = Command::new("chmod").args([*mode, *destination]).status();
    }

    let _ = Command::new("systemctl").arg("daemon-reload").status();
    let secondary_enabled = Command::new("systemctl")
        .args(["enable", SECONDARY_QMI_SERVICE_NAME])
        .status()
        .is_ok_and(|status| status.success());
    if !secondary_enabled {
        return "QCM410 resources installed but native-bearer service could not be enabled"
            .to_string();
    }

    let mut timer_enable = Command::new("systemctl");
    timer_enable.arg("enable");
    if restart_now {
        timer_enable.arg("--now");
    }
    let timer_enabled = timer_enable
        .arg(MODEM_RECOVERY_TIMER_NAME)
        .status()
        .is_ok_and(|status| status.success());
    if !timer_enabled {
        return "QCM410 resources installed but recovery timer could not be enabled".to_string();
    }

    if !restart_now {
        return "QCM410 native-bearer and recovery resources installed; activation deferred"
            .to_string();
    }

    let _ = Command::new("systemctl")
        .args(["stop", "ModemManager.service"])
        .status();
    let secondary_started = Command::new("systemctl")
        .args(["restart", SECONDARY_QMI_SERVICE_NAME])
        .status()
        .is_ok_and(|status| status.success());
    let _ = Command::new("systemctl")
        .args(["restart", "ModemManager.service"])
        .status();

    if secondary_started {
        "QCM410 native-bearer and recovery resources installed and activated".to_string()
    } else {
        "QCM410 resources installed; native-bearer initializer skipped or failed".to_string()
    }
}
