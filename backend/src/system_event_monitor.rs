use crate::system_event::{codes, severity, status, SystemEventEmitter};
use crate::utils::{read_disk_info, read_memory_info, read_network_interfaces, sample_cpu_usage};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tracing::warn;
use zbus::Connection;

const MONITOR_INTERVAL: Duration = Duration::from_secs(60);
const CPU_TRIGGER_SAMPLES: u32 = 5;
const CPU_RECOVER_SAMPLES: u32 = 2;
const MEMORY_TRIGGER_SAMPLES: u32 = 5;
const MEMORY_RECOVER_SAMPLES: u32 = 2;
const TEMP_TRIGGER_SAMPLES: u32 = 1;
const TEMP_RECOVER_SAMPLES: u32 = 2;
const DISK_TRIGGER_SAMPLES: u32 = 1;
const DISK_RECOVER_SAMPLES: u32 = 1;
const INTERFACE_TRIGGER_SAMPLES: u32 = 3;
const INTERFACE_RECOVER_SAMPLES: u32 = 2;
const CONNECTIVITY_TRIGGER_SAMPLES: u32 = 3;
const CONNECTIVITY_RECOVER_SAMPLES: u32 = 2;
const GIB: u64 = 1024 * 1024 * 1024;
const LOW_DISK_ABSOLUTE_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Default)]
struct AlarmCounter {
    active: bool,
    bad_count: u32,
    good_count: u32,
}

enum AlarmTransition {
    Triggered,
    Recovered,
}

impl AlarmCounter {
    fn update(
        &mut self,
        bad: bool,
        recovered: bool,
        trigger_samples: u32,
        recover_samples: u32,
    ) -> Option<AlarmTransition> {
        if self.active {
            if recovered {
                self.good_count += 1;
            } else {
                self.good_count = 0;
            }
            if self.good_count >= recover_samples {
                self.active = false;
                self.bad_count = 0;
                self.good_count = 0;
                return Some(AlarmTransition::Recovered);
            }
            return None;
        }

        if bad {
            self.bad_count += 1;
        } else {
            self.bad_count = 0;
        }
        if self.bad_count >= trigger_samples {
            self.active = true;
            self.bad_count = 0;
            self.good_count = 0;
            return Some(AlarmTransition::Triggered);
        }
        None
    }
}

#[derive(Default)]
struct ResourceMonitorState {
    cpu: AlarmCounter,
    memory: AlarmCounter,
    temperature: AlarmCounter,
    disks: HashMap<String, AlarmCounter>,
    interface_errors: HashMap<String, AlarmCounter>,
    previous_interface_errors: HashMap<String, u64>,
    ipv4: AlarmCounter,
    ipv6: AlarmCounter,
}

pub fn spawn_system_event_monitor(
    emitter: Arc<SystemEventEmitter>,
    dbus_conn: Arc<Connection>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(MONITOR_INTERVAL).await;
        let mut state = ResourceMonitorState::default();
        loop {
            sample_resources(&emitter, dbus_conn.as_ref(), &mut state).await;
            tokio::time::sleep(MONITOR_INTERVAL).await;
        }
    })
}

async fn sample_resources(
    emitter: &SystemEventEmitter,
    dbus_conn: &Connection,
    state: &mut ResourceMonitorState,
) {
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_CPU_HIGH,
        codes::RESOURCE_CPU_RECOVERED,
    ) {
        sample_cpu(emitter, &mut state.cpu).await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_MEMORY_HIGH,
        codes::RESOURCE_MEMORY_RECOVERED,
    ) {
        sample_memory(emitter, &mut state.memory).await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_TEMPERATURE_HIGH,
        codes::RESOURCE_TEMPERATURE_RECOVERED,
    ) {
        sample_temperature(emitter, &mut state.temperature).await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_DISK_LOW,
        codes::RESOURCE_DISK_RECOVERED,
    ) {
        sample_disks(emitter, &mut state.disks).await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_INTERFACE_ERRORS_INCREASED,
        codes::RESOURCE_INTERFACE_ERRORS_RECOVERED,
    ) {
        sample_interfaces(
            emitter,
            dbus_conn,
            &mut state.interface_errors,
            &mut state.previous_interface_errors,
        )
        .await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_IPV4_CONNECTIVITY_FAILED,
        codes::RESOURCE_IPV4_CONNECTIVITY_RECOVERED,
    ) {
        sample_connectivity(emitter, &mut state.ipv4, false).await;
    }
    if system_event_pair_enabled(
        emitter,
        codes::RESOURCE_IPV6_CONNECTIVITY_FAILED,
        codes::RESOURCE_IPV6_CONNECTIVITY_RECOVERED,
    ) {
        sample_connectivity(emitter, &mut state.ipv6, true).await;
    }
}

fn system_event_pair_enabled(
    emitter: &SystemEventEmitter,
    trigger_code: &str,
    recover_code: &str,
) -> bool {
    emitter.is_enabled(trigger_code) || emitter.is_enabled(recover_code)
}

async fn sample_cpu(emitter: &SystemEventEmitter, alarm: &mut AlarmCounter) {
    let Ok(usage) = sample_cpu_usage().await else {
        return;
    };
    match alarm.update(
        usage >= 90.0,
        usage <= 75.0,
        CPU_TRIGGER_SAMPLES,
        CPU_RECOVER_SAMPLES,
    ) {
        Some(AlarmTransition::Triggered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_CPU_HIGH,
                    severity::WARNING,
                    status::TRIGGERED,
                    "cpu",
                    format!("CPU 使用率持续高负载，当前 {:.0}%", usage),
                )
                .await;
        }
        Some(AlarmTransition::Recovered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_CPU_RECOVERED,
                    severity::INFO,
                    status::RECOVERED,
                    "cpu",
                    format!("CPU 使用率已恢复，当前 {:.0}%", usage),
                )
                .await;
        }
        None => {}
    }
}

async fn sample_memory(emitter: &SystemEventEmitter, alarm: &mut AlarmCounter) {
    let Ok((total, available, _, _)) = read_memory_info() else {
        return;
    };
    if total == 0 {
        return;
    }
    let used_percent = (total.saturating_sub(available) as f64 / total as f64) * 100.0;
    match alarm.update(
        used_percent >= 90.0,
        used_percent <= 80.0,
        MEMORY_TRIGGER_SAMPLES,
        MEMORY_RECOVER_SAMPLES,
    ) {
        Some(AlarmTransition::Triggered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_MEMORY_HIGH,
                    severity::WARNING,
                    status::TRIGGERED,
                    "memory",
                    format!("内存持续高占用，当前 {:.0}%", used_percent),
                )
                .await;
        }
        Some(AlarmTransition::Recovered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_MEMORY_RECOVERED,
                    severity::INFO,
                    status::RECOVERED,
                    "memory",
                    format!("内存占用已恢复，当前 {:.0}%", used_percent),
                )
                .await;
        }
        None => {}
    }
}

async fn sample_temperature(emitter: &SystemEventEmitter, alarm: &mut AlarmCounter) {
    let Some((name, value)) = hottest_temperature() else {
        return;
    };
    match alarm.update(
        value >= 75.0,
        value <= 65.0,
        TEMP_TRIGGER_SAMPLES,
        TEMP_RECOVER_SAMPLES,
    ) {
        Some(AlarmTransition::Triggered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_TEMPERATURE_HIGH,
                    severity::WARNING,
                    status::TRIGGERED,
                    name.clone(),
                    format!("设备温度过高，{} 当前 {:.1}°C", name, value),
                )
                .await;
        }
        Some(AlarmTransition::Recovered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_TEMPERATURE_RECOVERED,
                    severity::INFO,
                    status::RECOVERED,
                    name.clone(),
                    format!("设备温度已恢复，{} 当前 {:.1}°C", name, value),
                )
                .await;
        }
        None => {}
    }
}

async fn sample_disks(emitter: &SystemEventEmitter, alarms: &mut HashMap<String, AlarmCounter>) {
    for disk in read_disk_info() {
        if disk.total_bytes == 0 {
            continue;
        }
        let available_percent = (disk.available_bytes as f64 / disk.total_bytes as f64) * 100.0;
        let alarm = alarms.entry(disk.mount_point.clone()).or_default();
        match alarm.update(
            available_percent <= 10.0 || disk.available_bytes <= LOW_DISK_ABSOLUTE_BYTES,
            available_percent >= 15.0 && disk.available_bytes >= 2 * GIB,
            DISK_TRIGGER_SAMPLES,
            DISK_RECOVER_SAMPLES,
        ) {
            Some(AlarmTransition::Triggered) => {
                emitter
                    .emit_code(
                        codes::RESOURCE_DISK_LOW,
                        severity::WARNING,
                        status::TRIGGERED,
                        disk.mount_point.clone(),
                        format!(
                            "磁盘空间不足，{} 可用 {:.1}%",
                            disk.mount_point, available_percent
                        ),
                    )
                    .await;
            }
            Some(AlarmTransition::Recovered) => {
                emitter
                    .emit_code(
                        codes::RESOURCE_DISK_RECOVERED,
                        severity::INFO,
                        status::RECOVERED,
                        disk.mount_point.clone(),
                        format!(
                            "磁盘空间已恢复，{} 可用 {:.1}%",
                            disk.mount_point, available_percent
                        ),
                    )
                    .await;
            }
            None => {}
        }
    }
}

async fn sample_interfaces(
    emitter: &SystemEventEmitter,
    dbus_conn: &Connection,
    alarms: &mut HashMap<String, AlarmCounter>,
    previous_errors: &mut HashMap<String, u64>,
) {
    let interfaces = match read_network_interfaces(Some(dbus_conn)).await {
        Ok(interfaces) => interfaces,
        Err(err) => {
            warn!(error = %err, "System event monitor failed to read network interfaces");
            return;
        }
    };

    for iface in interfaces {
        if iface.name == "lo" {
            continue;
        }
        let total_errors = iface.rx_errors.saturating_add(iface.tx_errors);
        let previous = previous_errors.insert(iface.name.clone(), total_errors);
        let increased = previous
            .map(|previous| total_errors > previous)
            .unwrap_or(false);
        let alarm = alarms.entry(iface.name.clone()).or_default();
        match alarm.update(
            increased,
            !increased,
            INTERFACE_TRIGGER_SAMPLES,
            INTERFACE_RECOVER_SAMPLES,
        ) {
            Some(AlarmTransition::Triggered) => {
                emitter
                    .emit_code(
                        codes::RESOURCE_INTERFACE_ERRORS_INCREASED,
                        severity::WARNING,
                        status::TRIGGERED,
                        iface.name.clone(),
                        format!(
                            "{} 错误包增长，rx_errors={}, tx_errors={}",
                            iface.name, iface.rx_errors, iface.tx_errors
                        ),
                    )
                    .await;
            }
            Some(AlarmTransition::Recovered) => {
                emitter
                    .emit_code(
                        codes::RESOURCE_INTERFACE_ERRORS_RECOVERED,
                        severity::INFO,
                        status::RECOVERED,
                        iface.name.clone(),
                        format!("{} 错误包增长已停止", iface.name),
                    )
                    .await;
            }
            None => {}
        }
    }
}

async fn sample_connectivity(emitter: &SystemEventEmitter, alarm: &mut AlarmCounter, ipv6: bool) {
    let success = ping_connectivity(ipv6).await;
    match alarm.update(
        !success,
        success,
        CONNECTIVITY_TRIGGER_SAMPLES,
        CONNECTIVITY_RECOVER_SAMPLES,
    ) {
        Some(AlarmTransition::Triggered) if ipv6 => {
            emitter
                .emit_code(
                    codes::RESOURCE_IPV6_CONNECTIVITY_FAILED,
                    severity::WARNING,
                    status::FAILED,
                    "ipv6",
                    "IPv6 连通性连续检测失败",
                )
                .await;
        }
        Some(AlarmTransition::Triggered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_IPV4_CONNECTIVITY_FAILED,
                    severity::WARNING,
                    status::FAILED,
                    "ipv4",
                    "IPv4 连通性连续检测失败",
                )
                .await;
        }
        Some(AlarmTransition::Recovered) if ipv6 => {
            emitter
                .emit_code(
                    codes::RESOURCE_IPV6_CONNECTIVITY_RECOVERED,
                    severity::INFO,
                    status::RECOVERED,
                    "ipv6",
                    "IPv6 连通性已恢复",
                )
                .await;
        }
        Some(AlarmTransition::Recovered) => {
            emitter
                .emit_code(
                    codes::RESOURCE_IPV4_CONNECTIVITY_RECOVERED,
                    severity::INFO,
                    status::RECOVERED,
                    "ipv4",
                    "IPv4 连通性已恢复",
                )
                .await;
        }
        None => {}
    }
}

async fn ping_connectivity(ipv6: bool) -> bool {
    let (cmd, target) = if ipv6 {
        ("ping6", "2400:3200::1")
    } else {
        ("ping", "223.5.5.5")
    };
    Command::new(cmd)
        .args(["-c", "1", "-W", "1", target])
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn hottest_temperature() -> Option<(String, f64)> {
    let entries = fs::read_dir("/sys/class/thermal").ok()?;
    let mut hottest: Option<(String, f64)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let Some(raw_temp) = fs::read_to_string(path.join("temp"))
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
        else {
            continue;
        };
        let temperature = if raw_temp > 1000.0 {
            raw_temp / 1000.0
        } else {
            raw_temp
        };
        let sensor_type = fs::read_to_string(path.join("type"))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or(name);
        match hottest {
            Some((_, current)) if current >= temperature => {}
            _ => hottest = Some((sensor_type, temperature)),
        }
    }

    hottest
}
