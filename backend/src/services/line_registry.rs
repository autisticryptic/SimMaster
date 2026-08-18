//! Per-modem/SIM runtime registry.
//!
//! Each stable hardware+SIM line owns one independent runtime. API handlers must
//! resolve a line before touching modem, IMS, data, or trunk state.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Instant,
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock as AsyncRwLock};
use zbus::Connection;

use crate::{
    connectivity::modems::ims::volte::{live::VolteLiveHandle, VolteRuntime, VolteRuntimeStatus},
    connectivity::modems::ims::vowifi::runtime::VowifiRuntime,
    hardware::cellular::data_proxy::{DataProxyRuntime, DataProxyTraffic},
    hardware::cellular::modem_manager::{discover_modem_bindings, ModemBinding},
    hardware::devices::qcm410::secondary_qmi_data::SecondaryDataRuntime,
    platform::config::{
        AccessPathKind, ConfigManager, ModemSlotObservation, TrunkProfileConfig, VoicePathPolicy,
    },
    platform::db::{Database, LineDataTrafficEntry},
    services::supplementary::{SupplementaryRuntime, SupplementarySnapshot},
    services::trunk::{
        access_router::VoiceAccessRouter,
        runtime::{TrunkRuntime, TrunkRuntimeStatus},
    },
};

/// Independent recovery bookkeeping for one physical line. Keeping this on
/// `LineRuntime` prevents a slow or unhealthy modem from consuming another
/// line's retry counters or cooldowns.
#[derive(Debug, Default)]
pub struct LineDataWatchdogState {
    pub searching_polls: u32,
    pub missing_data_polls: u32,
    pub last_register_attempt: Option<Instant>,
    pub last_connect_attempt: Option<Instant>,
}

impl LineDataWatchdogState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub struct LineRuntime {
    binding: RwLock<ModemBinding>,
    pub volte: Arc<VolteRuntime>,
    pub volte_live: VolteLiveHandle,
    /// Serializes every PDP/bearer transition on this physical SIM line.
    /// DATA6 and IMS use different QMI endpoints, but the baseband policy engine
    /// still rejects or deactivates sessions when both are started concurrently.
    pub bearer_operation_lock: Mutex<()>,
    pub volte_connect_lock: Mutex<()>,
    pub volte_retry_running: AtomicBool,
    /// This line's own VoWiFi runtime, bound to its `line_id` so its executor
    /// stages read that line's ePDG/DNS/proxy overrides and its tunnel gets its
    /// own TUN device. Several SIMs (different countries, different proxies) can
    /// therefore be connected at the same time.
    pub vowifi: Arc<VowifiRuntime>,
    pub vowifi_connect_lock: Mutex<()>,
    vowifi_restore_running: AtomicBool,
    vowifi_sms_listener_running: AtomicBool,
    ims_voice_listener_running: AtomicBool,
    pub voice_access: VoiceAccessRouter,
    pub trunk: Arc<TrunkRuntime>,
    pub supplementary: Arc<SupplementaryRuntime>,
    pub data_proxy: Arc<DataProxyRuntime>,
    /// Serializes and records the background data-health workflow for this line.
    /// The scheduler uses `try_lock`, so overlapping ticks are dropped instead
    /// of queuing repeated registration or bearer operations.
    pub data_watchdog: Mutex<LineDataWatchdogState>,
    /// Dedicated DATA6 bearer that feeds only this line's HTTP/SOCKS proxy.
    /// It is separate from the proxy listener because the bearer must remain
    /// alive while listeners are reconfigured.
    pub secondary_data: Arc<SecondaryDataRuntime>,
}

impl LineRuntime {
    fn new(
        binding: ModemBinding,
        volte: Arc<VolteRuntime>,
        volte_live: VolteLiveHandle,
        voice_policy: VoicePathPolicy,
    ) -> Self {
        let vowifi_operator =
            crate::connectivity::modems::ims::vowifi::operator::operator_link_for_line(
                &binding.line_id,
            );
        let voice_access = VoiceAccessRouter::new(
            voice_policy,
            vec![
                (AccessPathKind::Vowifi, vowifi_operator),
                (AccessPathKind::Volte, volte_live.operator_link()),
            ],
        );
        let operator = voice_access.operator_link();
        let vowifi = Arc::new(VowifiRuntime::for_line(&binding.line_id));
        let supplementary = Arc::new(SupplementaryRuntime::for_line(&binding.line_id));
        volte_live.bind_supplementary(Arc::clone(&supplementary));
        crate::connectivity::modems::ims::vowifi::operator::bind_supplementary_for_line(
            &binding.line_id,
            Arc::clone(&supplementary),
        );
        Self {
            binding: RwLock::new(binding),
            volte,
            volte_live,
            bearer_operation_lock: Mutex::new(()),
            volte_connect_lock: Mutex::new(()),
            volte_retry_running: AtomicBool::new(false),
            vowifi,
            vowifi_connect_lock: Mutex::new(()),
            vowifi_restore_running: AtomicBool::new(false),
            vowifi_sms_listener_running: AtomicBool::new(false),
            ims_voice_listener_running: AtomicBool::new(false),
            voice_access,
            trunk: Arc::new(TrunkRuntime::with_operator(operator)),
            supplementary,
            data_proxy: Arc::new(DataProxyRuntime::default()),
            data_watchdog: Mutex::new(LineDataWatchdogState::default()),
            secondary_data: Arc::new(SecondaryDataRuntime::default()),
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
            supplementary: self.supplementary.snapshot().await,
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

    pub fn begin_vowifi_sms_listener(&self) -> bool {
        self.vowifi_sms_listener_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn finish_vowifi_sms_listener(&self) {
        self.vowifi_sms_listener_running
            .store(false, Ordering::SeqCst);
    }

    pub fn begin_ims_voice_listener(&self) -> bool {
        self.ims_voice_listener_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn finish_ims_voice_listener(&self) {
        self.ims_voice_listener_running
            .store(false, Ordering::SeqCst);
    }

    /// Claim a complete VoWiFi restore workflow, including its settle and
    /// identity-gate delays. The connect mutex alone is too narrow because two
    /// reconcilers could otherwise sleep and probe the same SIM concurrently.
    pub fn begin_vowifi_restore(&self) -> bool {
        self.vowifi_restore_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn finish_vowifi_restore(&self) {
        self.vowifi_restore_running.store(false, Ordering::SeqCst);
    }

    pub fn vowifi_restore_in_progress(&self) -> bool {
        self.vowifi_restore_running.load(Ordering::SeqCst)
    }

    fn effective_trunk_profile(&self, profile: &TrunkProfileConfig) -> TrunkProfileConfig {
        let mut effective = profile.clone();
        if !self.binding().present {
            effective.enabled = false;
        }
        effective
    }

    pub async fn activate_trunk_profile(&self, profile: &TrunkProfileConfig) -> TrunkRuntimeStatus {
        let effective = self.effective_trunk_profile(profile);
        self.trunk.activate_profile(&effective).await;
        self.trunk.status().await
    }

    pub async fn reconcile_trunk_profile(
        &self,
        profile: &TrunkProfileConfig,
    ) -> TrunkRuntimeStatus {
        let effective = self.effective_trunk_profile(profile);
        self.trunk.reconcile_profile(&effective).await;
        self.trunk.status().await
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LineRuntimeStatus {
    pub modem: ModemBinding,
    pub volte: VolteRuntimeStatus,
    pub trunk: TrunkRuntimeStatus,
    pub supplementary: SupplementarySnapshot,
}

#[derive(Default)]
pub struct LineRuntimeRegistry {
    lines: AsyncRwLock<BTreeMap<String, Arc<LineRuntime>>>,
    config_manager: Option<Arc<ConfigManager>>,
    /// Used to restore each line's cumulative proxied-traffic counters when the
    /// line is first discovered, so totals survive a restart.
    database: Option<Arc<Database>>,
    /// Serializes periodic traffic flushes with an explicit session reset, so
    /// an in-flight flush cannot restore the just-cleared database row.
    traffic_persistence_lock: Mutex<()>,
}

impl LineRuntimeRegistry {
    pub fn new() -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            config_manager: None,
            database: None,
            traffic_persistence_lock: Mutex::new(()),
        }
    }

    pub fn with_config(config_manager: Arc<ConfigManager>, database: Arc<Database>) -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            config_manager: Some(config_manager),
            database: Some(database),
            traffic_persistence_lock: Mutex::new(()),
        }
    }

    /// Refresh presence and descriptors without discarding per-line runtime
    /// state. Missing lines remain addressable as offline entries so callers
    /// can tear them down and the same SIM can safely reappear after hotplug.
    pub async fn refresh(&self, conn: &Connection) -> zbus::Result<usize> {
        let mut discovered = match discover_modem_bindings(conn).await {
            Ok(bindings) => bindings,
            Err(error) => {
                tracing::warn!(error = %error, "ModemManager discovery unavailable; continuing with non-baseband lines");
                Vec::new()
            }
        };
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
        }

        // PC/SC readers are physical line anchors just like modem slots. A card
        // that is present is exposed automatically; the old standalone slot
        // configuration is consulted only for label and line-ID migration.
        if let Some(config_manager) = &self.config_manager {
            let legacy_slots = config_manager.get_standalone_sim_slots();
            match crate::hardware::devices::pcsc::discover_readers().await {
                Ok(readers) => {
                    let first_reader_order = discovered
                        .iter()
                        .map(|binding| binding.display_order)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    for reader in readers.into_iter().filter(|reader| reader.card_present) {
                        let reader_path = reader.selector.as_str();
                        let legacy_slot = legacy_slots.iter().find(|slot| {
                            slot.reader_path.trim() == reader_path
                                || slot.reader_path.trim() == reader.name
                                || slot.reader_path.trim() == reader.index.to_string()
                        });
                        let label = legacy_slot
                            .map(|slot| slot.label.trim())
                            .filter(|label| !label.is_empty())
                            .unwrap_or(reader.name.as_str());
                        let legacy_line_ids = legacy_slot
                            .map(|slot| {
                                vec![crate::hardware::cellular::modem_manager::reader_line_id(
                                    &slot.id,
                                    slot.uim_slot,
                                )]
                            })
                            .unwrap_or_default();
                        let identity = match crate::hardware::devices::pcsc::read_identity_async(
                            reader_path,
                        )
                        .await
                        {
                            Ok(identity) => Some(identity),
                            Err(error) => {
                                tracing::warn!(reader = %reader.name, error = %error, "Failed to read PC/SC SIM identity");
                                None
                            }
                        };
                        let sim_iccid = identity
                            .as_ref()
                            .map(|identity| identity.iccid.clone())
                            .unwrap_or_default();
                        let operator_id = identity
                            .as_ref()
                            .and_then(|identity| {
                                let mnc_length =
                                    identity.mnc_length.map(usize::from).unwrap_or_else(|| {
                                        if identity.imsi.starts_with("460") {
                                            2
                                        } else {
                                            3
                                        }
                                    });
                                (identity.imsi.len() >= 3 + mnc_length)
                                    .then(|| identity.imsi[..3 + mnc_length].to_string())
                            })
                            .unwrap_or_default();
                        let mut binding = crate::hardware::cellular::modem_manager::reader_binding(
                            &reader.name,
                            label,
                            reader_path,
                            1,
                            None,
                            true,
                            sim_iccid,
                            operator_id,
                            legacy_line_ids,
                        );
                        binding.display_order =
                            first_reader_order.saturating_add(reader.index.into());
                        if let Some(slot) = legacy_slot {
                            if let Err(error) = config_manager
                                .migrate_standalone_reader_references(&slot.id, &binding.line_id)
                            {
                                tracing::warn!(slot_id = %slot.id, line_id = %binding.line_id, error = %error, "Failed to migrate standalone reader references");
                            }
                        }
                        discovered.push(binding);
                    }
                }
                Err(error) => tracing::debug!(error = %error, "PC/SC reader discovery unavailable"),
            }

            // Preserve non-PC/SC legacy UIM adapters while their automatic
            // hardware discovery is being implemented. They remain headless
            // and do not require the removed reader management page.
            for slot in legacy_slots.iter().filter(|slot| {
                slot.enabled
                    && !slot.reader_path.trim().starts_with("pcsc://")
                    && slot.reader_path.trim().starts_with("/dev/")
            }) {
                let reader_path = slot.reader_path.trim();
                discovered.push(crate::hardware::cellular::modem_manager::reader_binding(
                    &slot.id,
                    &slot.label,
                    reader_path,
                    slot.uim_slot,
                    Some(reader_path.to_string()),
                    true,
                    String::new(),
                    String::new(),
                    Vec::new(),
                ));
            }

            for binding in &discovered {
                if let Err(error) = config_manager
                    .migrate_line_profile_aliases(&binding.line_id, &binding.legacy_line_ids)
                {
                    tracing::warn!(line_id = %binding.line_id, error = %error, "Failed to migrate line configuration aliases");
                }
                if let Some(database) = &self.database {
                    if let Err(error) = database.migrate_line_data_traffic_aliases(
                        &binding.line_id,
                        &binding.legacy_line_ids,
                    ) {
                        tracing::warn!(line_id = %binding.line_id, error = %error, "Failed to migrate line traffic aliases");
                    }
                }
            }
            let mut migration_lines = discovered.iter().collect::<Vec<_>>();
            migration_lines.sort_by(|left, right| {
                left.display_order
                    .cmp(&right.display_order)
                    .then_with(|| left.line_id.cmp(&right.line_id))
            });
            let line_ids = migration_lines
                .into_iter()
                .map(|binding| binding.line_id.clone())
                .collect::<Vec<_>>();
            if let Err(error) = config_manager.reconcile_line_profiles(&line_ids) {
                tracing::warn!(error = %error, "Failed to reconcile discovered line profiles");
            }
        }
        let mut lines = self.lines.write().await;
        for line in lines.values() {
            crate::connectivity::modems::ims::vowifi::live::forget_line_sim_device(
                &line.binding().line_id,
            );
            line.mark_absent();
        }
        for binding in discovered {
            // Tell the VoWiFi live layer which SIM device this line owns, so its
            // identity and authentication never use another modem's card.
            if binding.line_kind == "reader" && binding.model.starts_with("pcsc://") {
                crate::connectivity::modems::ims::vowifi::live::register_line_pcsc_reader(
                    &binding.line_id,
                    &binding.model,
                );
            } else {
                crate::connectivity::modems::ims::vowifi::live::register_line_sim_device(
                    &binding.line_id,
                    binding.qmi_device.as_deref().unwrap_or_default(),
                    binding.uim_slot,
                    &binding.modem_path,
                );
            }
            if let Some(line) = lines.get(&binding.line_id) {
                if let Some(config_manager) = &self.config_manager {
                    line.voice_access
                        .set_policy(config_manager.get_line_voice_path_policy(&binding.line_id));
                }
                line.replace_binding(binding);
                continue;
            }
            let runtime = Arc::new(VolteRuntime::new());
            let live = VolteLiveHandle::new();
            let line_id = binding.line_id.clone();
            let voice_policy = self
                .config_manager
                .as_ref()
                .map(|config| config.get_line_voice_path_policy(&line_id))
                .unwrap_or_default();
            let line = Arc::new(LineRuntime::new(binding, runtime, live, voice_policy));
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
            line.voice_access
                .set_policy(config_manager.get_line_voice_path_policy(&line.binding().line_id));
            line.reconcile_trunk_profile(&profile.trunk).await;
        }
    }

    /// Write every line's current traffic total to the database.
    ///
    /// The value written is the absolute total (persisted baseline + what this
    /// process has carried), so repeated flushes overwrite rather than
    /// accumulate. A crash between flushes therefore loses only the traffic
    /// since the last flush — it never double-counts.
    pub async fn flush_data_traffic(&self) {
        let _guard = self.traffic_persistence_lock.lock().await;
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
        let _guard = self.traffic_persistence_lock.lock().await;
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
            device_family: "generic_modem".to_string(),
            control_transport: "modemmanager_qmi_at".to_string(),
            primary_port: "wwan0mbim0".to_string(),
            qmi_device: Some("/dev/wwan0qmi0".to_string()),
            uim_slot: 1,
            sim_path: Some("/org/freedesktop/ModemManager1/SIM/0".to_string()),
            sim_iccid: "8986000000000000000".to_string(),
            sim_type: "physical".to_string(),
            esim_status: "unknown".to_string(),
            line_kind: "baseband".to_string(),
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
        let line = LineRuntime::new(
            binding("line-a", true),
            runtime,
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let status = line.status().await;
        assert_eq!(status.modem.line_id, "line-a");
        assert_eq!(line.vowifi.line_id(), "line-a");
        assert_eq!(status.volte.phase, "disabled");
        assert_eq!(status.trunk.phase, "disabled");
    }

    #[tokio::test]
    async fn absent_transition_does_not_change_stable_identity() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(
            binding("line-a", true),
            runtime,
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        line.mark_absent();
        assert_eq!(line.binding().line_id, "line-a");
        assert!(!line.binding().present);
    }

    #[test]
    fn absent_line_forces_only_the_runtime_trunk_profile_off() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(
            binding("line-a", false),
            runtime,
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let requested = TrunkProfileConfig {
            enabled: true,
            ..Default::default()
        };

        let effective = line.effective_trunk_profile(&requested);

        assert!(requested.enabled);
        assert!(!effective.enabled);
    }

    #[test]
    fn vowifi_restore_claim_is_exclusive_and_reusable() {
        let runtime = Arc::new(VolteRuntime::new());
        let line = LineRuntime::new(
            binding("line-a", true),
            runtime,
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );

        assert!(line.begin_vowifi_restore());
        assert!(line.vowifi_restore_in_progress());
        assert!(!line.begin_vowifi_restore());
        line.finish_vowifi_restore();
        assert!(line.begin_vowifi_restore());
    }

    #[tokio::test]
    async fn data_watchdog_state_is_independent_per_line() {
        let line_a = LineRuntime::new(
            binding("line-a", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let line_b = LineRuntime::new(
            binding("line-b", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );

        {
            let mut state = line_a.data_watchdog.lock().await;
            state.searching_polls = 4;
            state.missing_data_polls = 2;
            state.last_connect_attempt = Some(Instant::now());
        }

        let state_b = line_b.data_watchdog.lock().await;
        assert_eq!(state_b.searching_polls, 0);
        assert_eq!(state_b.missing_data_polls, 0);
        assert!(state_b.last_connect_attempt.is_none());
    }

    #[test]
    fn volte_runtime_and_operator_channels_are_independent_per_line() {
        let line_a = LineRuntime::new(
            binding("line-a", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let line_b = LineRuntime::new(
            binding("line-b", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        assert!(!Arc::ptr_eq(&line_a.volte, &line_b.volte));

        let operator_a = line_a.volte_live.operator_link();
        let operator_b = line_b.volte_live.operator_link();
        let _commands_a = operator_a.subscribe_commands();
        operator_a.set_ready(true);

        assert!(operator_a.is_available());
        assert!(!operator_b.is_available());
    }

    #[tokio::test]
    async fn trunk_and_supplementary_teardown_are_independent_per_line() {
        let line_a = LineRuntime::new(
            binding("line-a", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let line_b = LineRuntime::new(
            binding("line-b", true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        );
        let profile = TrunkProfileConfig {
            enabled: true,
            registration_mode: crate::platform::config::TrunkRegistrationMode::StaticPeer,
            asterisk_host: "127.0.0.1".to_string(),
            local_port: 5098,
            ..Default::default()
        };
        line_a.trunk.apply_profile(&profile).await;
        line_b.trunk.apply_profile(&profile).await;
        let line_b_generation = line_b.trunk.generation();
        let line_a_operator = line_a.voice_access.operator_link();
        let line_b_operator = line_b.voice_access.operator_link();
        line_a_operator.set_video_enabled(true);
        line_a_operator.media_metrics().record_rtp_to_asterisk(160);
        line_a
            .supplementary
            .begin_mwi_subscription(
                crate::connectivity::core::registration::ImsRegistrationAccess::Volte,
            )
            .await;
        line_b
            .supplementary
            .begin_mwi_subscription(
                crate::connectivity::core::registration::ImsRegistrationAccess::Vowifi,
            )
            .await;

        line_a
            .trunk
            .apply_profile(&TrunkProfileConfig::default())
            .await;
        line_a
            .supplementary
            .clear_registration(
                crate::connectivity::core::registration::ImsRegistrationAccess::Volte,
            )
            .await;

        assert_eq!(line_a.trunk.status().await.phase, "disabled");
        assert_eq!(line_b.trunk.status().await.phase, "configured");
        assert_eq!(line_b.trunk.generation(), line_b_generation);
        assert_eq!(line_a_operator.diagnostics().rtp_to_asterisk_packets, 1);
        assert_eq!(line_b_operator.diagnostics().rtp_to_asterisk_packets, 0);
        assert!(line_a_operator.video_enabled());
        assert!(!line_b_operator.video_enabled());
        assert!(
            !line_a
                .supplementary
                .snapshot()
                .await
                .mwi_capability
                .supported
        );
        assert!(
            line_b
                .supplementary
                .snapshot()
                .await
                .mwi_capability
                .supported
        );
    }
}
