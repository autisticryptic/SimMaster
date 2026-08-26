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
    time::{Duration, Instant},
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
    services::trunk::{
        access_router::VoiceAccessRouter,
        runtime::{TrunkRuntime, TrunkRuntimeStatus},
    },
    services::{
        supplementary::{SupplementaryRuntime, SupplementarySnapshot},
        ue_context::UeContext,
        ue_netcfg,
        ue_worker::{UeWorkerHandle, UeWorkerStatus},
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

/// Base suppression window applied after an IMS activation wedges the baseband.
const BASEBAND_WEDGE_BASE_COOLDOWN_SECS: u64 = 120;
/// Ceiling for the escalating window. A firmware that keeps dying on IMS
/// activation must not be retried more often than this.
const BASEBAND_WEDGE_MAX_COOLDOWN_SECS: u64 = 1800;

/// Memory of IMS activations that took the baseband down with them.
///
/// The MSM8916 firmware answers some IMS PDP activations with a modem subsystem
/// fatal (`dhcp_client_mgr.c:263`). That crash re-enumerates the modem, which
/// makes the line briefly absent, which resets the VoLTE snapshot -- including
/// the `manual_retry_available` flag the abort path had just set. The guard was
/// therefore erased by the very crash it exists to prevent, and automatic
/// restore re-issued the identical activation on its next pass.
///
/// Keeping the fact on `LineRuntime` instead survives that hotplug, and the
/// window doubles per consecutive crash so a reproducibly hostile firmware
/// backs off instead of restarting the baseband every minute.
#[derive(Debug, Default)]
struct BasebandWedgeState {
    /// When the most recent wedging activation was observed.
    observed_at: Option<Instant>,
    /// Consecutive wedges with no successful registration in between.
    consecutive: u32,
    /// A confirmed Qualcomm bam-dmux runtime-PM latch survives every in-process
    /// retry; only a full system reboot clears it. Keep this distinct from a
    /// transient activation crash so manual retries cannot hammer wwan0.
    permanent: bool,
}

impl BasebandWedgeState {
    fn cooldown(&self) -> Duration {
        let shift = self.consecutive.saturating_sub(1).min(4);
        let secs = BASEBAND_WEDGE_BASE_COOLDOWN_SECS
            .saturating_mul(1u64 << shift)
            .min(BASEBAND_WEDGE_MAX_COOLDOWN_SECS);
        Duration::from_secs(secs)
    }

    fn remaining(&self) -> Option<Duration> {
        let observed_at = self.observed_at?;
        self.cooldown().checked_sub(observed_at.elapsed())
    }
}

pub struct LineRuntime {
    binding: RwLock<ModemBinding>,
    /// Per-UE identity for this line. Every access leg (VoLTE, VoWiFi, data
    /// proxy, trunk) resolves through this context, which owns the line's
    /// Linux network namespace when isolation is enabled.
    pub ue: RwLock<UeContext>,
    /// Per-UE worker process. When isolation is enabled the worker is spawned
    /// inside the UE namespace (`setns`) and will host the UE's IMS/data
    /// sockets, so identical IPs/P-CSCF/xfrm state can never cross lines.
    pub ue_worker: UeWorkerHandle,
    pub volte: Arc<VolteRuntime>,
    pub volte_live: VolteLiveHandle,
    /// Serializes every PDP/bearer transition on this physical SIM line.
    /// DATA6 and IMS use different QMI endpoints, but the baseband policy engine
    /// still rejects or deactivates sessions when both are started concurrently.
    pub bearer_operation_lock: Mutex<()>,
    pub volte_connect_lock: Mutex<()>,
    pub volte_retry_running: AtomicBool,
    /// Suppression window for IMS activations that crash the baseband. Held
    /// here, not in the VoLTE snapshot, because the crash re-enumerates the
    /// modem and that hotplug resets the snapshot. See [`BasebandWedgeState`].
    baseband_wedge: RwLock<BasebandWedgeState>,
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
    /// Fingerprint of the last successfully applied UE egress plan.
    /// When the plan is unchanged across reconcile calls the worker
    /// net-config batch is skipped to avoid flooding the worker with
    /// redundant operations.
    egress_fingerprint: Mutex<Option<String>>,
    /// Serializes namespace/worker/socket-context transitions for this line.
    /// Refresh runs outside the registry map lock, so this per-line lock keeps
    /// a slow reconcile from racing a later refresh teardown or worker restart.
    ue_lifecycle_lock: Mutex<()>,
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
        let ue_context = UeContext::for_binding(
            &binding,
            &crate::platform::config::UeIsolationConfig::default(),
        );
        let line_id = binding.line_id.clone();
        let namespace = ue_context.namespace.clone();
        Self {
            binding: RwLock::new(binding),
            ue: RwLock::new(ue_context),
            ue_worker: UeWorkerHandle::for_line(&line_id, namespace),
            volte,
            volte_live,
            bearer_operation_lock: Mutex::new(()),
            volte_connect_lock: Mutex::new(()),
            volte_retry_running: AtomicBool::new(false),
            baseband_wedge: RwLock::new(BasebandWedgeState::default()),
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
            egress_fingerprint: Mutex::new(None),
            ue_lifecycle_lock: Mutex::new(()),
        }
    }

    pub fn binding(&self) -> ModemBinding {
        self.binding
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn ue(&self) -> UeContext {
        self.ue
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn replace_binding(&self, binding: ModemBinding) {
        *self
            .binding
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = binding;
        let binding = self.binding();
        let mut ue = self
            .ue
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ue.update_binding(&binding);
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
            ue: self.ue(),
            ue_worker: self.ue_worker.status().await,
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

    /// Record that an IMS activation on this line wedged the baseband, opening
    /// (or widening) the window during which it must not be retried.
    pub fn note_baseband_wedged(&self) -> Duration {
        let mut wedge = self
            .baseband_wedge
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wedge.consecutive = wedge.consecutive.saturating_add(1);
        wedge.observed_at = Some(Instant::now());
        wedge.cooldown()
    }

    pub fn note_baseband_wedged_permanent(&self) {
        let mut wedge = self
            .baseband_wedge
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wedge.permanent = true;
        wedge.observed_at = Some(Instant::now());
    }

    pub fn baseband_wedge_permanent(&self) -> bool {
        self.baseband_wedge
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .permanent
    }

    /// How long IMS activation is still suppressed on this line, if at all.
    ///
    /// Read by automatic restore before it starts a batch. Unlike the VoLTE
    /// snapshot this is not cleared by the hotplug that a wedging crash causes.
    pub fn baseband_wedge_remaining(&self) -> Option<Duration> {
        self.baseband_wedge
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remaining()
    }

    /// Clear the suppression window after IMS registers, so an occasional crash
    /// does not permanently inflate the backoff for a line that works.
    pub fn clear_baseband_wedge(&self) {
        let mut wedge = self
            .baseband_wedge
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !wedge.permanent {
            *wedge = BasebandWedgeState::default();
        }
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
    pub ue: UeContext,
    pub ue_worker: UeWorkerStatus,
    pub volte: VolteRuntimeStatus,
    pub trunk: TrunkRuntimeStatus,
    pub supplementary: SupplementarySnapshot,
}

/// The UE state prepared during a refresh but not visible to consumers until
/// the corresponding line binding is published.  Keeping this snapshot
/// private during namespace/worker operations prevents a reader from seeing a
/// new worker with the previous modem binding.
struct PreparedUePublication {
    ue: Option<UeContext>,
    worker: Option<UeWorkerHandle>,
    features: crate::services::ue_worker::UeWorkerFeatures,
    socket_context: Option<crate::connectivity::modems::ims::vowifi::live::LiveUeSocketContext>,
}

impl Default for PreparedUePublication {
    fn default() -> Self {
        Self {
            ue: None,
            worker: None,
            features: crate::services::ue_worker::UeWorkerFeatures::default(),
            socket_context: None,
        }
    }
}

/// Why a UE egress reconcile did not finish, and whether that justifies
/// dismantling the line's isolation.
///
/// The distinction is not cosmetic. The failure path tears down the DATA6
/// bearer, stops the worker and removes the namespace, and a refresh runs every
/// ten seconds. Treating a worker that simply has not finished its handshake as
/// a terminal failure therefore builds a self-sustaining loop: teardown, the
/// data watchdog rebuilds the bearer, the next refresh spawns a worker and
/// times out on it again. Each turn of that loop issues another QMI
/// stop/start pair at the baseband, and the firmware does not survive being
/// driven that way -- it dies in `dhcp_client_mgr.c`. A worker that is still
/// starting is expected, costs nothing to wait for, and must leave a healthy
/// bearer alone.
enum EgressError {
    /// The worker is absent or has not signalled ready yet. The next refresh
    /// finds it running; nothing is torn down.
    WorkerNotReady(String),
    /// The veth pair could not be created, or the worker rejected the
    /// configuration. The namespace cannot carry traffic, so the line has to
    /// fall back to the host path.
    Terminal(String),
}

impl EgressError {
    fn is_transient(&self) -> bool {
        matches!(self, Self::WorkerNotReady(_))
    }
}

impl std::fmt::Display for EgressError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerNotReady(message) | Self::Terminal(message) => formatter.write_str(message),
        }
    }
}

#[derive(Default)]
pub struct LineRuntimeRegistry {
    lines: AsyncRwLock<BTreeMap<String, Arc<LineRuntime>>>,
    /// Serializes hardware discovery passes without holding the registry write
    /// lock across worker, QMI, or namespace operations.
    refresh_lock: Mutex<()>,
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
            refresh_lock: Mutex::new(()),
            config_manager: None,
            database: None,
            traffic_persistence_lock: Mutex::new(()),
        }
    }

    pub fn with_config(config_manager: Arc<ConfigManager>, database: Arc<Database>) -> Self {
        Self {
            lines: AsyncRwLock::new(BTreeMap::new()),
            refresh_lock: Mutex::new(()),
            config_manager: Some(config_manager),
            database: Some(database),
            traffic_persistence_lock: Mutex::new(()),
        }
    }

    /// Refresh presence and descriptors without discarding per-line runtime
    /// state. Missing lines remain addressable as offline entries so callers
    /// can tear them down and the same SIM can safely reappear after hotplug.
    pub async fn refresh(&self, conn: &Connection) -> zbus::Result<usize> {
        // Several handlers and background watchers may refresh concurrently.
        // Keep discovery/reconciliation passes ordered, while the registry write
        // lock remains reserved for the short snapshot publication below.
        let _refresh_guard = self.refresh_lock.lock().await;
        let mut discovered = match discover_modem_bindings(conn).await {
            Ok(bindings) => bindings,
            Err(error) => {
                tracing::warn!(error = %error, "ModemManager discovery unavailable; continuing with non-baseband lines");
                Vec::new()
            }
        };
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

        // A modem/reader discovery pass can report the same stable line more
        // than once (for example when a legacy reader alias and its automatic
        // reader record overlap). Keep one deterministic binding; otherwise
        // two private runtimes would reconcile the same namespace and the
        // later map insert would orphan the first worker/veth pair. Compute
        // the conflict counts before deduplication: `physical_line_id` is
        // intentionally derived from the physical slot, so counting after
        // deduplication would erase the diagnostic flag for duplicate modem
        // objects sharing that slot.
        let mut physical_slot_counts = std::collections::HashMap::new();
        for binding in &discovered {
            *physical_slot_counts
                .entry((binding.hardware_key.clone(), binding.uim_slot))
                .or_insert(0usize) += 1;
        }
        let mut unique_bindings: BTreeMap<String, ModemBinding> = BTreeMap::new();
        for binding in discovered {
            if let Some(existing) = unique_bindings.get(&binding.line_id) {
                tracing::warn!(
                    line_id = %binding.line_id,
                    kept_model = %existing.model,
                    dropped_model = %binding.model,
                    "Duplicate modem binding discovered; keeping the first stable line"
                );
                continue;
            }
            unique_bindings.insert(binding.line_id.clone(), binding);
        }
        let mut discovered = unique_bindings.into_values().collect::<Vec<_>>();
        for binding in &mut discovered {
            binding.slot_conflict |= physical_slot_counts
                .get(&(binding.hardware_key.clone(), binding.uim_slot))
                .is_some_and(|count| *count > 1);
        }

        // Publish only the binding snapshot while holding the registry lock.
        // Namespace, worker, and bearer operations are intentionally deferred
        // until after the lock is released so status/API readers stay usable
        // while one modem is slow or unavailable.
        let discovered_ids = discovered
            .iter()
            .map(|binding| binding.line_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let (absent_lines, existing_lines, new_lines) = {
            let lines = self.lines.read().await;
            let mut existing_lines = Vec::new();
            let mut new_lines = Vec::new();
            for binding in discovered {
                if let Some(line) = lines.get(&binding.line_id) {
                    existing_lines.push((Arc::clone(line), binding));
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
                new_lines.push((line_id, line));
            }

            let absent_lines = lines
                .values()
                .filter(|line| !discovered_ids.contains(&line.binding().line_id))
                .cloned()
                .collect::<Vec<_>>();
            (absent_lines, existing_lines, new_lines)
        };

        // Restore a new line's persisted counters before publishing it to the
        // registry. The persistence lock prevents a periodic flush from seeing
        // the zero baseline and overwriting the stored cumulative total.
        {
            let _traffic_guard = self.traffic_persistence_lock.lock().await;
            if let Some(database) = &self.database {
                for (line_id, line) in &new_lines {
                    if let Ok(stored) = database.get_line_data_traffic(line_id) {
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
            }
        }

        // Keep new runtimes private until their namespace/worker/socket
        // context has been reconciled. `refresh_lock` keeps another refresh
        // from publishing the same line concurrently, and the line is not
        // visible to a traffic flush while this work is in progress.
        let mut prepared_new = Vec::with_capacity(new_lines.len());
        for (_, line) in &new_lines {
            let binding = line.binding();
            prepared_new.push(self.reconcile_ue_context(line, &binding).await);
        }

        let mut prepared_existing = Vec::with_capacity(existing_lines.len());
        for (line, binding) in &existing_lines {
            prepared_existing.push(self.reconcile_ue_context(line, binding).await);
        }

        // Publish the completed binding snapshot only after all namespace and
        // worker transitions have finished. Readers therefore keep seeing the
        // previous coherent binding while a refresh is in progress.
        {
            let _traffic_guard = self.traffic_persistence_lock.lock().await;
            let mut lines = self.lines.write().await;
            for ((line, binding), prepared) in existing_lines.iter().zip(prepared_existing) {
                if let Some(config_manager) = &self.config_manager {
                    line.voice_access
                        .set_policy(config_manager.get_line_voice_path_policy(&binding.line_id));
                }
                line.replace_binding(binding.clone());
                Self::publish_sim_device_mapping(binding);
                Self::publish_ue_context(line, &binding.line_id, prepared);
            }
            for line in &absent_lines {
                line.mark_absent();
            }
            for ((line_id, line), prepared) in new_lines.iter().zip(prepared_new) {
                Self::publish_sim_device_mapping(&line.binding());
                Self::publish_ue_context(line, line_id.as_str(), prepared);
                lines.insert(line_id.clone(), Arc::clone(line));
            }
        }

        let present_count = existing_lines
            .iter()
            .filter(|(_, binding)| binding.present)
            .count()
            + new_lines
                .iter()
                .filter(|(_, line)| line.binding().present)
                .count();

        // Shut down workers whose hardware anchor disappeared. This is also
        // deliberately outside the registry lock.
        for line in &absent_lines {
            let _lifecycle_guard = line.ue_lifecycle_lock.lock().await;
            let binding = line.binding();
            let worker = line.ue_worker.clone();
            // Stop any DATA6 bearer before shutting down the namespace worker.
            // Otherwise the retained QMI session can keep an interface bound
            // to a namespace that is about to disappear.
            line.secondary_data.stop().await;
            if worker.is_running().await {
                if let Err(error) = worker.shutdown().await {
                    tracing::warn!(
                        line_id = %binding.line_id,
                        error = %error,
                        "Failed to stop per-UE worker for absent line"
                    );
                }
            }
            self.teardown_ue_isolation_locked(line, &binding.line_id)
                .await;
            crate::connectivity::modems::ims::vowifi::live::forget_line_sim_device_mapping(
                &binding.line_id,
            );
        }
        Ok(present_count)
    }

    /// Publish the SIM/reader mapping only alongside the line binding. Keeping
    /// discovery-time mappings private until this point prevents consumers
    /// from combining a newly discovered reader with the previous binding
    /// while UE reconciliation is still in progress.
    fn publish_sim_device_mapping(binding: &ModemBinding) {
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

    /// Keep a line's UE context in sync with its binding and the isolation
    /// master switch. Namespace creation is idempotent; a failure only drops
    /// the isolation guarantee and leaves the existing host-namespace path
    /// fully functional.
    async fn reconcile_ue_context(
        &self,
        line: &LineRuntime,
        binding: &ModemBinding,
    ) -> PreparedUePublication {
        let _lifecycle_guard = line.ue_lifecycle_lock.lock().await;
        let isolation = self
            .config_manager
            .as_ref()
            .map(|config| config.get_ue_isolation())
            .unwrap_or_default();
        // Capture this before changing the registry.  A missing worker is
        // common on the legacy host path (isolation disabled, or the worker
        // failed before the first isolated VoWiFi runtime was published), and
        // must not tear down an otherwise healthy host VoWiFi session.  Only a
        // line that actually had the shared isolated socket context needs the
        // expensive live-runtime cleanup below.
        let had_ue_socket_context =
            crate::connectivity::modems::ims::vowifi::live::ue_socket_context_for_line(
                &binding.line_id,
            )
            .is_some();
        let mut ue = line.ue();
        ue.update_binding(binding);
        if let Err(error) = ue.ensure_netns(&isolation).await {
            tracing::warn!(
                line_id = %binding.line_id,
                error = %error,
                "Failed to prepare per-UE network namespace"
            );
        }
        let ue_ready = ue.isolation_enabled && ue.netns_ready;
        let worker = line.ue_worker.clone();
        // A worker spawned in this pass has not completed its handshake yet.
        // Reconciling its egress in the same pass would only wait out the ready
        // timeout, and the timeout is what used to dismantle the line.
        let mut worker_spawned_this_pass = false;
        if ue_ready && !worker.is_running().await {
            // Clear the egress fingerprint so the freshly spawned worker
            // receives its initial net-config even if the plan is unchanged.
            {
                let mut fp = line.egress_fingerprint.lock().await;
                *fp = None;
            }
            match worker.spawn().await {
                Ok(()) => worker_spawned_this_pass = true,
                Err(error) => {
                    tracing::warn!(
                        line_id = %binding.line_id,
                        error = %error,
                        "Failed to start per-UE worker inside its namespace"
                    );
                }
            }
        } else if !ue_ready && worker.is_running().await {
            // Release the DATA6 bearer first so its in-namespace address and
            // routes are removed over a control channel that is still up. Only
            // a worker-bound session needs this; a host-side one keeps running.
            if line.secondary_data.is_worker_bound().await {
                line.secondary_data.stop().await;
            }
            if let Err(error) = worker.shutdown().await {
                tracing::warn!(
                    line_id = %binding.line_id,
                    error = %error,
                    "Failed to stop per-UE worker after isolation was disabled"
                );
            }
            // Clear the egress fingerprint so re-enabling isolation
            // re-applies the net-config rather than skipping it.
            {
                let mut fp = line.egress_fingerprint.lock().await;
                *fp = None;
            }
        }
        let line_id = binding.line_id.clone();
        // Publish the worker through the generic UE registry.  Other access
        // legs (VoLTE, data proxy and the future 5G bearer) resolve the same
        // line owner instead of maintaining another per-module map.
        let worker_registration = if ue_ready && worker.is_running().await {
            Some(worker.clone())
        } else {
            None
        };
        let worker_available = worker_registration.is_some();
        // Operator RTP can only enter a worker after the 3GPP bearer itself
        // has moved there.  Warn once per process rather than on every line
        // refresh, which would bury real failures during regression runs.
        if isolation.trunk_sockets_gate_suppressed() {
            static SUPPRESSED_TRUNK_GATE: std::sync::Once = std::sync::Once::new();
            SUPPRESSED_TRUNK_GATE.call_once(|| {
                tracing::warn!(
                    "Ignoring trunk_sockets_in_worker until three_gpp_ims_sockets_in_worker is enabled"
                );
            });
        }
        let features = crate::services::ue_worker::UeWorkerFeatures {
            three_gpp_ims: isolation.three_gpp_ims_sockets_in_worker,
            data_proxy: isolation.data_proxy_in_worker,
            trunk_sockets: isolation.effective_trunk_sockets_in_worker(),
        };
        if !worker_available {
            // A cached TUN/SIP channel may still belong to the dead worker's
            // namespace. Tear the live access runtime down before allowing a
            // host-path retry, otherwise a host socket can bind a UE-only TUN
            // and reproduce ENODEV even though the context registry is clear.
            if had_ue_socket_context {
                crate::connectivity::modems::ims::vowifi::live::clear_live_runtime_for_line(
                    &line_id,
                )
                .await;
            }
        }
        if ue_ready && worker_spawned_this_pass {
            // Let the worker finish its handshake on its own time. Everything
            // this pass built -- namespace, veth, a running child -- stays, and
            // the next refresh reconciles the egress against a ready worker.
            tracing::debug!(
                line_id = %line_id,
                "UE worker just spawned; deferring egress apply to the next refresh"
            );
            return PreparedUePublication {
                ue: Some(ue),
                ..PreparedUePublication::default()
            };
        }
        if ue_ready {
            if let Err(error) = self.reconcile_ue_egress(line, &ue).await {
                // A worker that has not finished its handshake yet is not a
                // failure of this line's isolation, and the next refresh is ten
                // seconds away. Returning without publishing the worker leaves
                // the bearer, the namespace and the veth exactly as they are so
                // that retry is free. Tearing them down instead would make this
                // path self-sustaining: the data watchdog rebuilds DATA6, the
                // following refresh spawns a worker and times out on it again,
                // and every turn drives another QMI stop/start pair into a
                // baseband whose firmware does not survive that treatment.
                if error.is_transient() {
                    tracing::debug!(
                        line_id = %line_id,
                        error = %error,
                        "UE worker not ready for egress apply; retrying on the next refresh"
                    );
                    return PreparedUePublication {
                        ue: Some(ue),
                        ..PreparedUePublication::default()
                    };
                }
                // Egress preparation may have created a new worker, veth,
                // NAT rule or namespace before failing.  Falling back by
                // merely publishing `None` would orphan those resources and
                // leave a stale worker/data session alive.  Only clear the
                // live runtime when a UE socket context was already
                // published; a first isolated reconcile can still be serving
                // the legacy host path and must not tear that runtime down.
                if had_ue_socket_context {
                    crate::connectivity::modems::ims::vowifi::live::clear_live_runtime_for_line(
                        &line_id,
                    )
                    .await;
                }
                // Stop the DATA6 bearer while the worker is still alive: its
                // teardown deletes the in-namespace address and routes over
                // the control channel before moving the interface back to the
                // host.  Then stop the worker so the shared teardown can
                // remove a namespace nothing is running in, and clear the
                // registry/context entries, NAT, veth and egress fingerprint.
                // A host-side session is untouched -- the line is falling back
                // to the host path, which is exactly where that session lives.
                if line.secondary_data.is_worker_bound().await {
                    line.secondary_data.stop().await;
                }
                if worker.is_running().await {
                    if let Err(shutdown_error) = worker.shutdown().await {
                        tracing::warn!(
                            line_id = %line_id,
                            error = %shutdown_error,
                            "Failed to stop UE worker after egress reconcile failure"
                        );
                    }
                }
                self.teardown_ue_isolation_locked(line, &line_id).await;
                tracing::warn!(
                    line_id = %line_id,
                    error = %error,
                    "Failed to reconcile UE egress/worker net-config; falling back to host path"
                );
                return PreparedUePublication {
                    ue: Some(ue),
                    ..PreparedUePublication::default()
                };
            }
        } else {
            // Fall back to the host-namespace VoWiFi path and best-effort
            // remove all resources from a previous isolated run.
            self.teardown_ue_isolation_locked(line, &line_id).await;
            return PreparedUePublication {
                ue: Some(ue),
                ..PreparedUePublication::default()
            };
        }

        let socket_context = if isolation.vowifi_tun_in_namespace && worker_available {
            let plan = ue_netcfg::plan_veth(&ue.namespace, &isolation);
            Some(
                crate::connectivity::modems::ims::vowifi::live::LiveUeSocketContext {
                    namespace: ue.namespace.as_str().to_string(),
                    ue_veth: plan.ue_if,
                    worker: worker.clone(),
                },
            )
        } else {
            None
        };
        PreparedUePublication {
            ue: Some(ue),
            worker: worker_registration,
            features,
            socket_context,
        }
    }

    fn publish_ue_context(line: &LineRuntime, line_id: &str, prepared: PreparedUePublication) {
        if let Some(ue) = prepared.ue {
            *line
                .ue
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ue;
        }
        crate::services::ue_worker::register_line_worker(
            line_id,
            prepared.worker,
            prepared.features,
        );
        crate::connectivity::modems::ims::vowifi::live::register_line_ue_socket_context(
            line_id,
            prepared.socket_context,
        );
    }

    async fn teardown_ue_isolation(&self, line: &LineRuntime, line_id: &str) {
        let _lifecycle_guard = line.ue_lifecycle_lock.lock().await;
        self.teardown_ue_isolation_locked(line, line_id).await;
    }

    async fn teardown_ue_isolation_locked(&self, line: &LineRuntime, line_id: &str) {
        let isolation = self
            .config_manager
            .as_ref()
            .map(|config| config.get_ue_isolation())
            .unwrap_or_default();
        let ue = line.ue();
        // Only a DATA session that was actually migrated into the worker
        // namespace is tied to this lifecycle: its interface lives in a
        // namespace that is about to disappear. A host-side session belongs to
        // the legacy path, and stopping it here would tear down healthy
        // cellular data on every single line refresh whenever isolation is
        // disabled -- the watchdog rebuilds it, this runs again, and the bearer
        // churns instead of ever carrying traffic.
        if line.secondary_data.is_worker_bound().await {
            line.secondary_data.stop().await;
        }
        crate::services::ue_worker::register_line_worker(
            line_id,
            None,
            crate::services::ue_worker::UeWorkerFeatures::default(),
        );
        crate::connectivity::modems::ims::vowifi::live::register_line_ue_socket_context(
            line_id, None,
        );
        let plan = ue_netcfg::plan_veth(&ue.namespace, &isolation);
        if let Err(error) = crate::platform::netns::remove_host_veth_nat(plan.host_addr).await {
            tracing::debug!(
                line_id,
                host_addr = %plan.host_addr,
                error = %error,
                "No UE veth NAT rule to remove"
            );
        }
        if let Err(error) = crate::platform::netns::teardown_veth(&plan.host_if).await {
            tracing::debug!(
                line_id,
                host_if = %plan.host_if,
                error = %error,
                "No UE veth pair to tear down"
            );
        }
        if let Err(error) = ue.teardown_netns().await {
            tracing::debug!(
                line_id,
                namespace = %ue.namespace,
                error = %error,
                "No UE namespace to remove"
            );
        }
        *line.egress_fingerprint.lock().await = None;
    }

    /// Prepare the UE-side egress and ask the worker to apply it inside the
    /// namespace. The parent only creates the veth pair and configures the
    /// host side; the worker owns the UE side (address/link/default route).
    async fn reconcile_ue_egress(
        &self,
        line: &LineRuntime,
        ue: &UeContext,
    ) -> Result<(), EgressError> {
        use std::time::Duration;

        let isolation = self
            .config_manager
            .as_ref()
            .map(|config| config.get_ue_isolation())
            .unwrap_or_default();
        let plan = ue_netcfg::plan_veth(&ue.namespace, &isolation);
        crate::platform::netns::ensure_veth_pair_host_side(
            &ue.namespace,
            &plan.host_if,
            &plan.ue_if,
            plan.host_addr,
            plan.mtu,
        )
        .await
        .map_err(|error| EgressError::Terminal(error.to_string()))?;

        // Build a fingerprint from the plan + isolation settings so we can
        // skip the worker net-config batch when nothing has changed.
        let fingerprint = format!(
            "{}|{}|{}|{}|{}|{}",
            plan.host_if,
            plan.ue_if,
            plan.host_addr,
            plan.ue_addr,
            plan.mtu,
            isolation.vowifi_tun_in_namespace,
        );
        let worker = line.ue_worker.clone();
        if !worker.is_running().await {
            return Err(EgressError::WorkerNotReady(
                "UE worker is not running; skipping egress apply".to_string(),
            ));
        }
        worker
            .wait_ready(Duration::from_secs(5))
            .await
            .map_err(|error| EgressError::WorkerNotReady(error.to_string()))?;
        let unchanged =
            line.egress_fingerprint.lock().await.as_deref() == Some(fingerprint.as_str());
        if unchanged {
            return Ok(());
        }
        // A transport failure means the worker went away mid-batch, which the
        // next refresh resolves by respawning it. Only an answer that actually
        // rejected the configuration proves the namespace cannot carry traffic.
        let result = worker
            .apply_net_config(ue_netcfg::veth_ue_side_ops(&plan))
            .await
            .map_err(|error| EgressError::WorkerNotReady(error.to_string()))?;
        if !result.ok {
            return Err(EgressError::Terminal(
                result
                    .error
                    .unwrap_or_else(|| "net-config failed".to_string()),
            ));
        }
        // Persist the fingerprint so subsequent reconcile calls are no-ops.
        {
            let mut slot = line.egress_fingerprint.lock().await;
            *slot = Some(fingerprint);
        }
        // Host-side SNAT for the UE egress subnet. Worker-created sockets
        // inside the UE namespace egress through this veth pair; without
        // MASQUERADE their source address would not be routable on the host
        // primary interface. Best-effort: routing still works for equal-subnet
        // deployments, so a failure only degrades reachability logging.
        if let Err(error) = crate::platform::netns::ensure_host_veth_nat(plan.host_addr).await {
            tracing::warn!(
                line_id = %ue.ue_id,
                host_addr = %plan.host_addr,
                error = %error,
                "Failed to ensure UE veth host SNAT"
            );
        }
        // Stage 2b is deliberately gated: only with this flag do the VoWiFi
        // TUN and every IKE/SIP/RTP socket move into the UE namespace through
        // the worker. Disabling keeps the previous host-namespace path.
        tracing::info!(
            line_id = %ue.ue_id,
            netns = %ue.namespace,
            host_if = %plan.host_if,
            ue_if = %plan.ue_if,
            "UE egress veth configured by worker"
        );
        Ok(())
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

    fn wedge_line(line_id: &str) -> LineRuntime {
        LineRuntime::new(
            binding(line_id, true),
            Arc::new(VolteRuntime::new()),
            VolteLiveHandle::new(),
            VoicePathPolicy::default(),
        )
    }

    #[test]
    fn a_fresh_line_never_suppresses_ims_activation() {
        assert!(wedge_line("line-a").baseband_wedge_remaining().is_none());
    }

    #[test]
    fn the_wedge_window_survives_the_hotplug_the_crash_itself_causes() {
        // This is the regression that made the crash loop self-sustaining: the
        // modem subsystem restart re-enumerates the modem, the line goes absent,
        // and the VoLTE snapshot -- where the abort flag used to live -- is reset
        // by `disconnect_live_for_line`. The cooldown must outlive all of that.
        let line = wedge_line("line-a");
        line.note_baseband_wedged();
        line.mark_absent();
        assert!(
            line.baseband_wedge_remaining().is_some(),
            "re-enumeration must not clear the suppression window"
        );
    }

    #[test]
    fn consecutive_wedges_widen_the_window_instead_of_retrying_every_minute() {
        let mut state = BasebandWedgeState::default();
        let mut seen = Vec::new();
        for _ in 0..3 {
            state.consecutive = state.consecutive.saturating_add(1);
            seen.push(state.cooldown().as_secs());
        }
        assert_eq!(
            seen,
            vec![
                BASEBAND_WEDGE_BASE_COOLDOWN_SECS,
                BASEBAND_WEDGE_BASE_COOLDOWN_SECS * 2,
                BASEBAND_WEDGE_BASE_COOLDOWN_SECS * 4,
            ]
        );
    }

    #[test]
    fn a_permanently_allergic_baseband_backs_off_to_a_bounded_ceiling() {
        let mut state = BasebandWedgeState::default();
        state.consecutive = 64;
        assert_eq!(
            state.cooldown().as_secs(),
            BASEBAND_WEDGE_MAX_COOLDOWN_SECS,
            "the window must stop growing rather than suppress the line forever"
        );
    }

    #[test]
    fn registering_clears_the_backoff_so_one_bad_activation_is_not_permanent() {
        let line = wedge_line("line-a");
        line.note_baseband_wedged();
        line.note_baseband_wedged();
        line.clear_baseband_wedge();
        assert!(line.baseband_wedge_remaining().is_none());
        // The next crash starts from the base window again, not from where the
        // previous streak left off.
        assert_eq!(
            line.note_baseband_wedged().as_secs(),
            BASEBAND_WEDGE_BASE_COOLDOWN_SECS
        );
    }

    #[test]
    fn one_lines_wedge_never_suppresses_another_sim() {
        let a = wedge_line("line-a");
        let b = wedge_line("line-b");
        a.note_baseband_wedged();
        assert!(a.baseband_wedge_remaining().is_some());
        assert!(
            b.baseband_wedge_remaining().is_none(),
            "the cooldown is per physical line, not process-wide"
        );
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
    fn sim_mapping_is_derived_from_the_published_binding() {
        let line_id = "test-line-registry-published-sim";
        let binding = binding(line_id, true);

        LineRuntimeRegistry::publish_sim_device_mapping(&binding);

        let mapped = crate::connectivity::modems::ims::vowifi::live::sim_device_for_line(line_id);
        assert_eq!(mapped.qmi_device, "/dev/wwan0qmi0");
        assert_eq!(mapped.uim_slot, 1);
        assert_eq!(mapped.modem_path, binding.modem_path);
        crate::connectivity::modems::ims::vowifi::live::forget_line_sim_device_mapping(line_id);
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
