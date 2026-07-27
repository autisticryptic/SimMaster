//! Per-modem/SIM runtime registry.
//!
//! Legacy SimAdmin selected the first ModemManager object and stored one global
//! VoLTE runtime. This registry keeps one independent runtime per stable
//! hardware+SIM line while retaining a seed runtime for backwards-compatible
//! single-line API handlers.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock as AsyncRwLock};
use zbus::Connection;

use crate::{
    access::volte::{live::VolteLiveHandle, VolteRuntime, VolteRuntimeStatus},
    access::vowifi::runtime::VowifiRuntime,
    cellular::data_proxy::{DataProxyRuntime, DataProxyTraffic},
    cellular::modem_manager::{discover_modem_bindings, ModemBinding},
    infra::config::{ConfigManager, ModemSlotObservation},
    infra::db::{Database, LineDataTrafficEntry},
    trunk::runtime::{TrunkRuntime, TrunkRuntimeStatus},
};

pub struct LineRuntime {
    binding: RwLock<ModemBinding>,
    pub volte: Arc<VolteRuntime>,
    pub volte_live: VolteLiveHandle,
    pub volte_connect_lock: Mutex<()>,
    pub volte_retry_running: AtomicBool,
    /// This line's own VoWiFi runtime, bound to its `line_id` so its executor
    /// stages read that line's ePDG/DNS/proxy overrides and its tunnel gets its
    /// own TUN device. Several SIMs (different countries, different proxies) can
    /// therefore be connected at the same time.
    pub vowifi: Arc<VowifiRuntime>,
    pub vowifi_connect_lock: Mutex<()>,
    pub trunk: Arc<TrunkRuntime>,
    pub data_proxy: Arc<DataProxyRuntime>,
}

impl LineRuntime {
    fn new(binding: ModemBinding, volte: Arc<VolteRuntime>, volte_live: VolteLiveHandle) -> Self {
        let operator = volte_live.operator_link();
        let vowifi = Arc::new(VowifiRuntime::for_line(&binding.line_id));
        Self {
            binding: RwLock::new(binding),
            volte,
            volte_live,
            volte_connect_lock: Mutex::new(()),
            volte_retry_running: AtomicBool::new(false),
            vowifi,
            vowifi_connect_lock: Mutex::new(()),
            trunk: Arc::new(TrunkRuntime::with_operator(operator)),
            data_proxy: Arc::new(DataProxyRuntime::default()),
        }
    }

    pub fn binding(&self) -> ModemBinding {
        self.binding
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace_binding(&self, binding: ModemBinding) {
        *self
            .binding
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = binding;
    }

    fn mark_absent(&self) {
        self.binding
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .present = false;
    }

    pub async fn status(&self) -> LineRuntimeStatus {
        LineRuntimeStatus {
            modem: self.binding(),
            volte: self.volte.status().await,
            trunk: self.trunk.status().await,
        }
    }

    /// Claim the complete VoLTE recovery workflow, not just one connect call.
    /// This keeps automatic restore and the Web retry action from overlapping.
    pub fn begin_volte_retry(&self) -> bool {
        self.volte_retry_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn finish_volte_retry(&self) {
        self.volte_retry_running.store(false, Ordering::SeqCst);
    }

    pub fn volte_retry_in_progress(&self) -> bool {
        self.volte_retry_running.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineRuntimeStatus {
    pub modem: ModemBinding,
    pub volte: VolteRuntimeStatus,
    pub trunk: TrunkRuntimeStatus,
}

#[derive(Default)]
pub struct LineRuntimeRegistry {
    lines: AsyncRwLock<BTreeMap<String, Arc<LineRuntime>>>,
    seed_runtime: Arc<VolteRuntime>,
    seed_claimed: AtomicBool,
    config_manager: Option<Arc<ConfigManager>>,
    /// Used to restore each line's cumulative proxied-traffic counters when the
    /// line is first discovered, so totals survive a restart.
    database: Option<Arc<Database>>,
}

impl LineRuntimeRegistry {
    pub fn new(seed_runtime: Arc<VolteRuntime>) -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            seed_runtime,
            seed_claimed: AtomicBool::new(false),
            config_manager: None,
            database: None,
        }
    }

    pub fn with_config(
        seed_runtime: Arc<VolteRuntime>,
        config_manager: Arc<ConfigManager>,
        database: Arc<Database>,
    ) -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            seed_runtime,
            seed_claimed: AtomicBool::new(false),
            config_manager: Some(config_manager),
            database: Some(database),
        }
    }

    /// Refresh presence and descriptors without discarding per-line runtime
    /// state. Missing lines remain addressable as offline entries so callers
    /// can tear them down and the same SIM can safely reappear after hotplug.
    pub async fn refresh(&self, conn: &Connection) -> zbus::Result<usize> {
        let mut discovered = discover_modem_bindings(conn).await?;
        let mut physical_slot_counts = std::collections::HashMap::new();
        for binding in &discovered {
            *physical_slot_counts
                .entry((binding.hardware_key.clone(), binding.uim_slot))
                .or_insert(0usize) += 1;
        }
        for binding in &mut discovered {
            binding.slot_conflict = physical_slot_counts
                .get(&(binding.hardware_key.clone(), binding.uim_slot))
                .is_some_and(|count| *count > 1);
        }
        if let Some(config_manager) = &self.config_manager {
            let observations = discovered
                .iter()
                .map(|binding| ModemSlotObservation {
                    slot_id: binding.hardware_key.clone(),
                    legacy_hardware_keys: binding.legacy_hardware_keys.clone(),
                    equipment_identifier: binding.equipment_identifier.clone(),
                    uim_slot: binding.uim_slot,
                })
                .collect::<Vec<_>>();
            if let Ok(slots) = config_manager.reconcile_modem_slots(&observations) {
                for binding in &mut discovered {
                    let slot_key = format!("{}#uim{}", binding.hardware_key, binding.uim_slot);
                    if let Some(slot) = slots.get(&slot_key) {
                        binding.display_order = slot.order;
                        binding.slot_label = slot.label.clone();
                    }
                }
            }
            for binding in &discovered {
                let _ = config_manager
                    .migrate_line_profile_aliases(&binding.line_id, &binding.legacy_line_ids);
            }
        }
        let mut lines = self.lines.write().await;
        for line in lines.values() {
            line.mark_absent();
        }
        for binding in discovered {
            // Tell the VoWiFi live layer which reader this line owns, so its SIM
            // authentication and IKE identity run against that line's card rather
            // than whichever modem happens to be first.
            if let Some(qmi_device) = binding.qmi_device.as_deref() {
                crate::access::vowifi::live::register_line_sim_device(
                    &binding.line_id,
                    qmi_device,
                    binding.uim_slot,
                    &binding.modem_path,
                );
            }
            if let Some(line) = lines.get(&binding.line_id) {
                line.replace_binding(binding);
                continue;
            }
            let is_seed = !self.seed_claimed.swap(true, Ordering::SeqCst);
            let runtime = if is_seed {
                Arc::clone(&self.seed_runtime)
            } else {
                Arc::new(VolteRuntime::new())
            };
            let live = if is_seed {
                VolteLiveHandle::legacy_shared()
            } else {
                VolteLiveHandle::new()
            };
            let line_id = binding.line_id.clone();
            let line = Arc::new(LineRuntime::new(binding, runtime, live));
            // Seed the traffic counters from disk the first time we see a line,
            // so the reported totals are cumulative rather than per-boot.
            if let Some(database) = &self.database {
                if let Ok(stored) = database.get_line_data_traffic(&line_id) {
                    line.data_proxy
                        .restore_persisted_traffic(DataProxyTraffic {
                            uplink_bytes: stored.uplink_bytes,
                            downlink_bytes: stored.downlink_bytes,
                            total_connections: stored.total_connections,
                            active_connections: 0,
                        })
                        .await;
                }
            }
            lines.insert(line_id, line);
        }
        Ok(lines.values().filter(|line| line.binding().present).count())
    }

    pub async fn get(&self, line_id: &str) -> Option<Arc<LineRuntime>> {
        self.lines.read().await.get(line_id).cloned()
    }

    pub async fn all(&self) -> Vec<Arc<LineRuntime>> {
        let mut lines = self
            .lines
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        lines.sort_by(|left, right| {
            let left = left.binding();
            let right = right.binding();
            left.display_order
                .cmp(&right.display_order)
                .then_with(|| right.present.cmp(&left.present))
                .then_with(|| left.line_id.cmp(&right.line_id))
        });
        lines
    }

    pub async fn primary(&self) -> Option<Arc<LineRuntime>> {
        self.all()
            .await
            .into_iter()
            .find(|line| line.binding().present)
    }

    pub async fn for_modem_path(&self, modem_path: &str) -> Option<Arc<LineRuntime>> {
        self.lines
            .read()
            .await
            .values()
            .find(|line| {
                let binding = line.binding();
                binding.present && binding.modem_path == modem_path
            })
            .cloned()
    }

    pub async fn statuses(&self) -> Vec<LineRuntimeStatus> {
        let lines = self.all().await;
        let mut statuses = Vec::with_capacity(lines.len());
        for line in lines {
            statuses.push(line.status().await);
        }
        statuses
    }

    pub async fn sync_trunk_profiles(&self, config_manager: &ConfigManager) {
        for line in self.all().await {
            let profile = config_manager.get_line_profile(&line.binding().line_id);
            line.trunk.reconcile_profile(&profile.trunk).await;
        }
    }

    /// Write every line's current traffic total to the database.
    ///
    /// The value written is the absolute total (persisted baseline + what this
    /// process has carried), so repeated flushes overwrite rather than
    /// accumulate. A crash between flushes therefore loses only the traffic
    /// since the last flush — it never double-counts.
    pub async fn flush_data_traffic(&self) {
        let Some(database) = &self.database else {
            return;
        };
        for line in self.all().await {
            let line_id = line.binding().line_id;
            let traffic = line.data_proxy.traffic().await;
            let entry = LineDataTrafficEntry {
                uplink_bytes: traffic.uplink_bytes,
                downlink_bytes: traffic.downlink_bytes,
                total_connections: traffic.total_connections,
            };
            if let Err(error) = database.set_line_data_traffic(&line_id, &entry) {
                tracing::warn!(line_id = %line_id, error = %error, "Failed to persist line data traffic");
            }
        }
    }

    /// Zero one line's traffic, in memory and on disk.
    pub async fn reset_data_traffic(&self, line_id: &str) -> Option<DataProxyTraffic> {
        let line = self.get(line_id).await?;
        let traffic = line.data_proxy.reset_traffic().await;
        if let Some(database) = &self.database {
            if let Err(error) = database.clear_line_data_traffic(line_id) {
                tracing::warn!(line_id = %line_id, error = %error, "Failed to clear line data traffic");
            }
        }
        Some(traffic)
    }

    pub async fn present_count(&self) -> usize {
        self.lines
            .read()
            .await
            .values()
            .filter(|line| line.binding().present)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(line_id: &str, present: bool) -> ModemBinding {
        ModemBinding {
            line_id: line_id.to_string(),
            display_order: 1,
            slot_label: "基带 1".to_string(),
            slot_source: "test".to_string(),
            slot_stable: true,
            slot_conflict: false,
            modem_id: "0".to_string(),
            modem_path: "/org/freedesktop/ModemManager1/Modem/0".to_string(),
            manufacturer: "test".to_string(),
            model: "test".to_string(),
            primary_port: "wwan0mbim0".to_string(),
            qmi_device: Some("/dev/wwan0qmi0".to_string()),
            uim_slot: 1,
            sim_path: Some("/org/freedesktop/ModemManager1/SIM/0".to_string()),
            sim_iccid: "8986000000000000000".to_string(),
            operator_id: "46000".to_string(),
            state: "registered".to_string(),
            present,
            hardware_key: "test-hardware".to_string(),
            equipment_identifier: "test-equipment".to_string(),
            legacy_hardware_keys: Vec::new(),
            legacy_line_ids: Vec::new(),
        }
    }

    #[tokio::test]
    async fn line_status_keeps_runtime_and_binding_together() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(binding("line-a", true), runtime, VolteLiveHandle::new());
        let status = line.status().await;
        assert_eq!(status.modem.line_id, "line-a");
        assert_eq!(status.volte.phase, "disabled");
        assert_eq!(status.trunk.phase, "disabled");
    }

    #[test]
    fn absent_transition_does_not_change_stable_identity() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(binding("line-a", true), runtime, VolteLiveHandle::new());
        line.mark_absent();
        assert_eq!(line.binding().line_id, "line-a");
        assert!(!line.binding().present);
    }
}
