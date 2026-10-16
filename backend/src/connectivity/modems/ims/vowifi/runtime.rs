use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::{Mutex, RwLock};
use zbus::Connection;

use super::{
    diagnostics::{self, VowifiProfileMatchResponse, VowifiReadiness, VowifiStatusResponse},
    executor::{
        readiness_key_for_stage, ExecutorStage, ExecutorStageRequest, ExecutorStageResult,
        ExecutorStageStatus, LiveExecutorGateReport, LiveRuntimeExecutor, NoopRuntimeExecutor,
        RuntimeExecutor, RuntimeExecutorReport,
    },
    flow,
    identity::VowifiSimIdentity,
    restore::RestoreProgress,
};
use crate::{
    connectivity::core::access_network::ImsAccessNetworkRuntime,
    hardware::cellular::control::sim_identity_for_modem,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePhase {
    NotStarted,
    IdentityReady,
    ProfileMatched,
    Failed,
}

impl RuntimePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimePhase::NotStarted => "not_started",
            RuntimePhase::IdentityReady => "identity_ready",
            RuntimePhase::ProfileMatched => "profile_matched",
            RuntimePhase::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub phase: RuntimePhase,
    pub profile: VowifiProfileMatchResponse,
    pub executor: RuntimeExecutorReport,
    pub live_readiness: RuntimeLiveReadiness,
    pub degraded_reason: Option<String>,
    pub restore: RestoreProgress,
    pub diagnostics: crate::connectivity::core::registration_diagnostics::DiagnosticsState,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            phase: RuntimePhase::NotStarted,
            profile: VowifiProfileMatchResponse::default(),
            executor: NoopRuntimeExecutor.describe(),
            live_readiness: RuntimeLiveReadiness::default(),
            degraded_reason: None,
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeLiveReadiness {
    pub sim_auth_ready: bool,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    pub ims_registered: bool,
    pub sms_ready: bool,
    pub voice_ready: bool,
    // Only a real refresh result with the original installed lease can restore
    // these former service flags. Neither config nor diagnostic facts can.
    refresh_prior: Option<[bool; 3]>,
    retained_until: Option<Instant>,
    register_recovery: Option<super::executor::RegisterRecovery>,
}

impl RuntimeLiveReadiness {
    fn normalize_protocol_prerequisites(&mut self) {
        if self.retained_until.is_some_and(|expiry| expiry <= Instant::now()) {
            self.ims_registered = false; self.sms_ready = false; self.voice_ready = false;
            self.refresh_prior = None; self.retained_until = None;
        }
        if self.epdg_ready
            || self.ike_ready
            || self.child_sa_ready
            || self.esp_ready
            || self.ims_registered
            || self.sms_ready
            || self.voice_ready
        {
            self.sim_auth_ready = true;
        }
        if self.child_sa_ready
            || self.esp_ready
            || self.ims_registered
            || self.sms_ready
            || self.voice_ready
        {
            self.ike_ready = true;
        }
        if self.esp_ready || self.ims_registered || self.sms_ready || self.voice_ready {
            self.epdg_ready = true;
            self.child_sa_ready = true;
        }
        if self.ims_registered || self.sms_ready || self.voice_ready {
            self.esp_ready = true;
        }
        if self.sms_ready || self.voice_ready {
            self.ims_registered = true;
        }
    }
}

#[derive(Clone)]
pub struct VowifiRuntime {
    snapshot: Arc<RwLock<RuntimeSnapshot>>,
    live_refresh: Arc<Mutex<LiveRefreshState>>,
    live_generation: Arc<AtomicU64>,
    registration_revocation_serial: Arc<AtomicU64>,
    diagnostic_serial: Arc<AtomicU64>,
    /// Which line this runtime serves. Threaded into every executor stage so the
    /// stage picks up this line's ePDG/DNS/proxy overrides — two SIMs on different
    /// operators (and different proxies) can therefore run concurrently without
    /// reading each other's settings.
    line_id: Arc<str>,
    /// Per-line serving-cell identity shared with the VoLTE runtime. The live
    /// REGISTER path reads only this handle and never opens its own D-Bus
    /// connection, so concurrent lines cannot borrow each other's PLMN/cell.
    access_network: ImsAccessNetworkRuntime,
}

#[derive(Debug)]
struct LiveRefreshState {
    last_finished: Option<Instant>,
}

impl VowifiRuntime {
    /// Build a runtime bound to one line, so its executor stages use that line's
    /// network overrides.
    pub fn for_line(line_id: impl AsRef<str>) -> Self {
        Self::for_line_with_gate_and_access_network(
            line_id,
            LiveExecutorGateReport::from_environment(),
            ImsAccessNetworkRuntime::default(),
        )
    }

    pub fn for_line_with_access_network(
        line_id: impl AsRef<str>,
        access_network: ImsAccessNetworkRuntime,
    ) -> Self {
        Self::for_line_with_gate_and_access_network(
            line_id,
            LiveExecutorGateReport::from_environment(),
            access_network,
        )
    }

    pub fn for_line_with_gate(line_id: impl AsRef<str>, live_gate: LiveExecutorGateReport) -> Self {
        Self::for_line_with_gate_and_access_network(
            line_id,
            live_gate,
            ImsAccessNetworkRuntime::default(),
        )
    }

    pub fn for_line_with_gate_and_access_network(
        line_id: impl AsRef<str>,
        live_gate: LiveExecutorGateReport,
        access_network: ImsAccessNetworkRuntime,
    ) -> Self {
        let line_id = line_id.as_ref().trim();
        assert!(!line_id.is_empty(), "VoWiFi runtime requires a line_id");
        let snapshot = RuntimeSnapshot {
            executor: LiveRuntimeExecutor::from_gate(live_gate).describe(),
            ..Default::default()
        };
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            live_refresh: Arc::new(Mutex::new(LiveRefreshState {
                last_finished: None,
            })),
            live_generation: Arc::new(AtomicU64::new(0)),
            registration_revocation_serial: Arc::new(AtomicU64::new(0)),
            diagnostic_serial: Arc::new(AtomicU64::new(0)),
            line_id: Arc::from(line_id),
            access_network,
        }
    }

    /// The line this runtime serves.
    pub fn line_id(&self) -> &str {
        &self.line_id
    }

    pub fn access_network(&self) -> &ImsAccessNetworkRuntime {
        &self.access_network
    }

    /// Local cache fencing only; this does not inspect or start an access.
    pub(crate) fn native_observation_generation(&self) -> u64 {
        self.live_generation.load(Ordering::Acquire)
    }

    /// Read the SIM identity of the modem this runtime is bound to. A runtime
    /// created with `for_line` resolves its own modem from the line registry, so
    /// line B never matches a carrier profile from line A's card.
    async fn read_bound_sim_identity(
        &self,
        conn: &Connection,
    ) -> Option<crate::hardware::cellular::control::SimIdentity> {
        let modem_path = super::live::sim_device_for_line(&self.line_id).modem_path;
        if modem_path.is_empty() {
            return None;
        }
        sim_identity_for_modem(conn, &modem_path).await
    }

    pub async fn refresh_identity(&self, conn: &Connection) -> RuntimeSnapshot {
        let next = match self.read_bound_sim_identity(conn).await {
            Some(identity) => {
                let identity = VowifiSimIdentity::from_modem(&identity);
                let presented_imsi =
                    super::live::effective_imsi_for_line(&self.line_id, identity.imsi());
                let identity = if presented_imsi != identity.imsi() {
                    identity.with_presented_imsi(presented_imsi)
                } else {
                    identity
                };
                let pinned = super::live::line_pinned_profile_id(&self.line_id);
                let profile =
                    match_profile_for_runtime(&self.line_id, &identity, pinned.as_deref());
                let previous = self.snapshot().await;
                let same_profile = same_matched_profile(&previous.profile, &profile);
                let live_readiness = previous.live_readiness_for_profile(
                    &self.line_id,
                    &identity,
                    pinned.as_deref(),
                );
                RuntimeSnapshot {
                    phase: if profile.matched {
                        RuntimePhase::ProfileMatched
                    } else if identity.present() {
                        RuntimePhase::IdentityReady
                    } else {
                        RuntimePhase::NotStarted
                    },
                    profile,
                    executor: previous.executor,
                    live_readiness,
                    degraded_reason: if same_profile {
                        previous.degraded_reason
                    } else {
                        None
                    },
                    diagnostics: if same_profile { previous.diagnostics.clone() } else { Default::default() },
                    restore: previous.restore,
                }
            }
            None => reset_identity_preserving_runtime(self.snapshot().await),
        };

        *self.snapshot.write().await = next.clone();
        next
    }

    pub async fn refresh_identity_with_timeout(
        &self,
        conn: &Connection,
        timeout_after: Duration,
    ) -> RuntimeSnapshot {
        match tokio::time::timeout(timeout_after, self.refresh_identity(conn)).await {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let mut snapshot = self.snapshot().await;
                snapshot.phase = RuntimePhase::Failed;
                snapshot.degraded_reason = Some("identity_refresh_timeout".to_string());
                *self.snapshot.write().await = snapshot.clone();
                snapshot
            }
        }
    }

    pub async fn snapshot(&self) -> RuntimeSnapshot {
        let mut snapshot = self.snapshot.read().await.clone();
        snapshot.diagnostics.discard_stale(self.live_generation.load(Ordering::SeqCst));
        snapshot.live_readiness.normalize_protocol_prerequisites();
        snapshot
    }

    pub(crate) fn same_diagnostics_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.live_generation, &other.live_generation)
    }

    pub(crate) async fn begin_ims_diagnostics(&self) -> Option<crate::connectivity::core::registration_diagnostics::DiagnosticsToken> {
        self.begin_ims_diagnostics_at(self.live_generation.load(Ordering::SeqCst)).await
    }

    pub(crate) async fn begin_ims_diagnostics_at(&self, generation: u64) -> Option<crate::connectivity::core::registration_diagnostics::DiagnosticsToken> {
        if self.live_generation.load(Ordering::SeqCst) != generation { return None; }
        let serial = self.diagnostic_serial.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |id| id.checked_add(1)).ok()?;
        let token = crate::connectivity::core::registration_diagnostics::DiagnosticsToken { generation, serial };
        let mut snapshot = self.snapshot.write().await;
        if self.live_generation.load(Ordering::SeqCst) != token.generation { return None; }
        snapshot.diagnostics.begin(token).then_some(token)
    }

    pub(crate) async fn current_ims_diagnostics_token(&self) -> Option<crate::connectivity::core::registration_diagnostics::DiagnosticsToken> {
        self.snapshot.read().await.diagnostics.token(self.live_generation.load(Ordering::SeqCst))
    }

    pub(crate) async fn observe_ims_diagnostics(&self,
        token: crate::connectivity::core::registration_diagnostics::DiagnosticsToken,
        change: impl FnOnce(&mut crate::connectivity::core::registration_diagnostics::DiagnosticsState),
    ) {
        self.snapshot.write().await.diagnostics.apply(token, self.live_generation.load(Ordering::SeqCst), change);
    }

    #[cfg(test)]
    pub(crate) async fn seed_registered_for_reg_event_test(&self) {
        let mut snapshot = self.snapshot.write().await;
        snapshot.profile.matched = true;
        snapshot.live_readiness.ims_registered = true;
        snapshot.live_readiness.sms_ready = true;
        snapshot.live_readiness.voice_ready = true;
        snapshot.live_readiness.normalize_protocol_prerequisites();
    }

    pub async fn reset_runtime(&self, reason: impl Into<String>) -> RuntimeSnapshot {
        self.live_generation.fetch_add(1, Ordering::SeqCst);
        let mut snapshot = RuntimeSnapshot::default();
        snapshot.executor = self.snapshot.read().await.executor.clone();
        snapshot.degraded_reason = Some(reason.into());
        *self.snapshot.write().await = snapshot.clone();
        snapshot
    }

    /// Invalidate only the IMS registration portion of a live WLAN access leg.
    ///
    /// A proactive REGISTER refresh must be allowed to retry over the existing
    /// ePDG/IKE/Child-SA/ESP path. Calling `reset_runtime` here would tear down
    /// the bearer on the first transient SIP failure and would also interrupt
    /// an active operator session. The next connect pass therefore starts at
    /// `ImsRegister` while retaining the already established lower layers.
    pub async fn prepare_live_ims_registration_refresh(
        &self,
        reason: impl Into<String>,
    ) -> RuntimeSnapshot {
        let mut snapshot = self.snapshot.write().await;
        if !snapshot.profile.matched { return snapshot.clone(); }
        snapshot.live_readiness.normalize_protocol_prerequisites();
        let readiness = &mut snapshot.live_readiness;
        readiness.refresh_prior = readiness.ims_registered.then_some([readiness.ims_registered, readiness.sms_ready, readiness.voice_ready]);
        readiness.retained_until = None;
        readiness.register_recovery = None;
        snapshot.phase = RuntimePhase::ProfileMatched;
        snapshot.live_readiness.ims_registered = false;
        snapshot.live_readiness.sms_ready = false;
        snapshot.live_readiness.voice_ready = false;
        snapshot.live_readiness.normalize_protocol_prerequisites();
        snapshot.degraded_reason = Some(reason.into());
        snapshot.clone()
    }

    /// A registrar loss is not a refresh-in-progress: never let a late
    /// temporary refresh result restore the just-revoked binding.
    pub(crate) async fn revoke_live_ims_registration(&self, reason: &str) -> RuntimeSnapshot {
        let mut snapshot = self.snapshot.write().await;
        self.registration_revocation_serial.fetch_add(1, Ordering::SeqCst);
        snapshot.phase = RuntimePhase::ProfileMatched;
        snapshot.live_readiness.ims_registered = false;
        snapshot.live_readiness.sms_ready = false;
        snapshot.live_readiness.voice_ready = false;
        snapshot.live_readiness.refresh_prior = None;
        snapshot.live_readiness.retained_until = None;
        snapshot.live_readiness.register_recovery = None;
        snapshot.degraded_reason = Some(reason.to_owned());
        snapshot.clone()
    }

    pub async fn refresh_live_readiness(
        &self,
        db: Option<&crate::platform::db::Database>,
    ) -> RuntimeSnapshot {
        self.refresh_live_readiness_with_stage_timeout(db, Duration::from_secs(30))
            .await
    }

    pub async fn refresh_status_readiness_with_stage_timeout(
        &self,
        db: Option<&crate::platform::db::Database>,
        stage_timeout: Duration,
    ) -> RuntimeSnapshot {
        self.refresh_live_readiness_controlled(
            db,
            stage_timeout,
            Duration::from_secs(5),
            LiveRefreshScope::StatusProbe,
        )
        .await
    }

    pub async fn refresh_live_readiness_with_stage_timeout(
        &self,
        db: Option<&crate::platform::db::Database>,
        stage_timeout: Duration,
    ) -> RuntimeSnapshot {
        self.refresh_live_readiness_controlled(
            db,
            stage_timeout,
            Duration::from_secs(20),
            LiveRefreshScope::Connect,
        )
        .await
    }

    pub async fn connect_live_with_stage_timeout(
        &self,
        db: Option<&crate::platform::db::Database>,
        stage_timeout: Duration,
    ) -> RuntimeSnapshot {
        let _refresh =
            match tokio::time::timeout(Duration::from_secs(8), self.live_refresh.lock()).await {
                Ok(refresh) => refresh,
                Err(_) => {
                    let mut snapshot = self.snapshot().await;
                    snapshot.degraded_reason = Some("live_connect_already_running".to_string());
                    return snapshot;
                }
            };

        loop {
            let before = self.snapshot().await;
            if !before.profile.matched
                || live_refresh_stages_for(&before.live_readiness, LiveRefreshScope::Connect)
                    .is_empty()
            {
                return before;
            }

            let next = self
                .refresh_live_readiness_once(db, stage_timeout, LiveRefreshScope::Connect)
                .await;
            if next.degraded_reason.is_some()
                || next.readiness() == before.readiness()
                || live_refresh_stages_for(&next.live_readiness, LiveRefreshScope::Connect)
                    .is_empty()
            {
                return next;
            }
        }
    }

    async fn refresh_live_readiness_controlled(
        &self,
        db: Option<&crate::platform::db::Database>,
        stage_timeout: Duration,
        min_interval: Duration,
        scope: LiveRefreshScope,
    ) -> RuntimeSnapshot {
        let Ok(mut refresh) = self.live_refresh.try_lock() else {
            return self.snapshot().await;
        };
        if refresh
            .last_finished
            .is_some_and(|finished| finished.elapsed() < min_interval)
        {
            return self.snapshot().await;
        }

        let snapshot = self
            .refresh_live_readiness_once(db, stage_timeout, scope)
            .await;
        refresh.last_finished = Some(Instant::now());
        snapshot
    }

    async fn refresh_live_readiness_once(
        &self,
        db: Option<&crate::platform::db::Database>,
        stage_timeout: Duration,
        scope: LiveRefreshScope,
    ) -> RuntimeSnapshot {
        let generation = self.live_generation.load(Ordering::SeqCst);
        let revocation_serial = self.registration_revocation_serial.load(Ordering::SeqCst);
        let snapshot = self.snapshot().await;
        if !snapshot.profile.matched {
            return snapshot;
        }

        let executor = LiveRuntimeExecutor::from_gate_with_access_network(
            snapshot.executor.live_gate.clone(),
            self.access_network.clone(),
        );
        let mut next = snapshot;
        let previous_degraded_reason = next.degraded_reason.clone();
        next.degraded_reason = None;
        let profile_id = next
            .profile
            .profile
            .as_ref()
            .map(|profile| profile.profile_id.to_string());
        let plmn = next
            .profile
            .profile
            .as_ref()
            .map(|profile| profile.plmn.to_string());

        let stages = live_refresh_stages_for(&next.live_readiness, scope);
        if stages.is_empty() && scope == LiveRefreshScope::StatusProbe {
            next.degraded_reason = previous_degraded_reason;
        }

        for stage in stages {
            let request = ExecutorStageRequest {
                stage,
                profile_id: profile_id.clone(),
                plmn: plmn.clone(),
                trace_id: match scope {
                    LiveRefreshScope::StatusProbe => "runtime-status-probe".to_string(),
                    LiveRefreshScope::Connect => "runtime-connect".to_string(),
                },
                line_id: self.line_id.to_string(),
            };

            // LOG STAGE START TO DB
            if let Some(database) = db {
                if scope == LiveRefreshScope::Connect {
                    let (event_type, phase, level) = match stage {
                        ExecutorStage::SimAuth => ("identity_refresh", "identity_read", "info"),
                        ExecutorStage::Epdg => ("dns_resolve", "dns_resolve", "info"),
                        ExecutorStage::Ike => ("ike_sa_init", "ike_negotiation", "info"),
                        ExecutorStage::ImsRegister => ("ims_register", "ims_register", "info"),
                        ExecutorStage::Sms => ("sms_binding", "sms_binding", "info"),
                        ExecutorStage::Voice => ("voice_binding", "voice_binding", "info"),
                        _ => ("", "", ""),
                    };
                    if !event_type.is_empty() {
                        let _ = database.insert_vowifi_runtime_event(
                            crate::platform::db::NewVowifiRuntimeEvent {
                                line_id: self.line_id.as_ref(),
                                trace_id: Some("runtime-connect"),
                                level,
                                phase,
                                profile_id: profile_id.as_deref(),
                                event_type,
                                detail_json: "{}",
                            },
                        );
                    }
                }
            }

            let result =
                match tokio::time::timeout(stage_timeout, super::live::with_registration_diagnostics(
                    self, generation, executor.run_stage(request),
                )).await {
                    Ok(result) => result,
                    Err(_) => ExecutorStageResult {
                        stage: stage.as_str(),
                        status: ExecutorStageStatus::Failed.as_str(),
                        readiness_key: readiness_key_for_stage(stage),
                        reason: Some(format!("{}_stage_timeout", stage.as_str())),
                        soak_observation: None,
                        register_recovery: None,
                    },
                };

            // LOG STAGE END TO DB
            if let Some(database) = db {
                if scope == LiveRefreshScope::Connect {
                    let completed = result.status == "completed";
                    if completed {
                        let events = match stage {
                            ExecutorStage::SimAuth => vec![
                                ("identity_refresh", "identity_ready", "success"),
                                ("profile_search", "profile_search", "info"),
                                ("profile_match", "profile_matched", "success"),
                            ],
                            ExecutorStage::Epdg => {
                                vec![("dns_resolved", "dns_resolved", "success")]
                            }
                            ExecutorStage::Ike => vec![("ike_established", "ike_ready", "success")],
                            ExecutorStage::Esp => vec![("esp_ready", "esp_ready", "success")],
                            ExecutorStage::ImsRegister => {
                                vec![("ims_registered", "ims_registered", "success")]
                            }
                            ExecutorStage::Sms => vec![("sms_ready", "sms_ready", "success")],
                            ExecutorStage::Voice => vec![("voice_ready", "voice_ready", "success")],
                            _ => vec![],
                        };
                        for (event_type, phase, level) in events {
                            let _ = database.insert_vowifi_runtime_event(
                                crate::platform::db::NewVowifiRuntimeEvent {
                                    line_id: self.line_id.as_ref(),
                                    trace_id: Some("runtime-connect"),
                                    level,
                                    phase,
                                    profile_id: profile_id.as_deref(),
                                    event_type,
                                    detail_json: "{}",
                                },
                            );
                        }
                    } else {
                        let (event_type, phase) = match stage {
                            ExecutorStage::SimAuth => ("identity_failed", "identity_failed"),
                            ExecutorStage::Epdg => ("dns_failed", "dns_failed"),
                            ExecutorStage::Ike | ExecutorStage::ChildSa | ExecutorStage::Esp => {
                                ("ike_failed", "ike_failed")
                            }
                            ExecutorStage::ImsRegister => {
                                ("ims_register_rejected", "ims_register_rejected")
                            }
                            ExecutorStage::Sms => ("sms_binding_failed", "sms_binding_failed"),
                            ExecutorStage::Voice => {
                                ("voice_binding_failed", "voice_binding_failed")
                            }
                            _ => ("", ""),
                        };
                        if !event_type.is_empty() {
                            let detail_json = if let Some(ref reason) = result.reason {
                                serde_json::json!({ "reason": reason }).to_string()
                            } else {
                                "{}".to_string()
                            };
                            let _ = database.insert_vowifi_runtime_event(
                                crate::platform::db::NewVowifiRuntimeEvent {
                                    line_id: self.line_id.as_ref(),
                                    trace_id: Some("runtime-connect"),
                                    level: "error",
                                    phase,
                                    profile_id: profile_id.as_deref(),
                                    event_type,
                                    detail_json: &detail_json,
                                },
                            );
                        }
                    }
                }
            }

            if self.live_generation.load(Ordering::SeqCst) != generation {
                super::live::clear_live_runtime_for_line(self.line_id()).await;
                return self.snapshot().await;
            }
            if self.registration_revocation_serial.load(Ordering::SeqCst) != revocation_serial {
                return self.snapshot().await;
            }
            let terminal = result.status != "completed";
            next.apply_stage_result(stage, &result);
            if terminal {
                break;
            }
        }

        if self.live_generation.load(Ordering::SeqCst) != generation {
            super::live::clear_live_runtime_for_line(self.line_id()).await;
            return self.snapshot().await;
        }
        self.publish_live_stages(generation, revocation_serial, next).await
    }

    async fn publish_live_stages(&self, generation: u64, revocation_serial: u64, mut next: RuntimeSnapshot) -> RuntimeSnapshot {
        let mut current = self.snapshot.write().await;
        if self.live_generation.load(Ordering::SeqCst) != generation
            || self.registration_revocation_serial.load(Ordering::SeqCst) != revocation_serial { return current.clone(); }
        // The live adapters publish observations while run_stage awaits. Do
        // not overwrite their actual facts with the pre-stage snapshot clone.
        next.diagnostics = current.diagnostics.clone();
        *current = next.clone();
        next
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveRefreshScope {
    StatusProbe,
    Connect,
}

#[cfg(test)]
fn live_refresh_stages(readiness: &RuntimeLiveReadiness) -> Vec<ExecutorStage> {
    live_refresh_stages_for(readiness, LiveRefreshScope::Connect)
}

fn live_refresh_stages_for(
    readiness: &RuntimeLiveReadiness,
    scope: LiveRefreshScope,
) -> Vec<ExecutorStage> {
    if !readiness.sim_auth_ready && scope == LiveRefreshScope::Connect {
        return vec![ExecutorStage::SimAuth];
    }
    if !readiness.epdg_ready {
        if scope == LiveRefreshScope::Connect {
            return vec![ExecutorStage::Esp];
        }
        return vec![ExecutorStage::Epdg];
    }
    if scope == LiveRefreshScope::StatusProbe {
        return Vec::new();
    }
    if !readiness.ike_ready || !readiness.child_sa_ready || !readiness.esp_ready {
        return vec![ExecutorStage::Esp];
    }
    let mut stages = Vec::new();
    if !readiness.ims_registered {
        stages.push(ExecutorStage::ImsRegister);
    }
    if !readiness.sms_ready {
        stages.push(ExecutorStage::Sms);
    }
    if !readiness.voice_ready {
        stages.push(ExecutorStage::Voice);
    }
    stages
}

fn same_matched_profile(
    previous: &VowifiProfileMatchResponse,
    current: &VowifiProfileMatchResponse,
) -> bool {
    let previous_profile_id = previous.profile.as_ref().map(|profile| profile.profile_id);
    let current_profile_id = current.profile.as_ref().map(|profile| profile.profile_id);
    previous_profile_id.is_some() && previous_profile_id == current_profile_id
}

fn match_profile_for_runtime(
    line_id: &str,
    identity: &VowifiSimIdentity,
    pinned_profile_id: Option<&str>,
) -> VowifiProfileMatchResponse {
    if let Some(selected) = super::live::live_profile_selection(line_id) {
        diagnostics::match_selected_profile_for_line(
            identity,
            selected.profile,
            &selected.source,
            selected.fallback_reason,
        )
    } else {
        diagnostics::match_profile_for_line(identity, pinned_profile_id)
    }
}

fn reset_identity_preserving_runtime(previous: RuntimeSnapshot) -> RuntimeSnapshot {
    if previous.profile.matched {
        return previous;
    }

    RuntimeSnapshot {
        phase: RuntimePhase::NotStarted,
        profile: VowifiProfileMatchResponse::default(),
        executor: previous.executor,
        live_readiness: RuntimeLiveReadiness::default(),
        degraded_reason: previous.degraded_reason,
        restore: previous.restore,
        diagnostics: Default::default(),
    }
}

impl RuntimeSnapshot {
    fn live_readiness_for_profile(
        &self,
        line_id: &str,
        identity: &VowifiSimIdentity,
        pinned_profile_id: Option<&str>,
    ) -> RuntimeLiveReadiness {
        if !identity.present() {
            return RuntimeLiveReadiness::default();
        }

        let previous_profile_id = self
            .profile
            .profile
            .as_ref()
            .map(|profile| profile.profile_id);
        let current_profile = match_profile_for_runtime(line_id, identity, pinned_profile_id);
        let current_profile_id = current_profile
            .profile
            .as_ref()
            .map(|profile| profile.profile_id);

        if previous_profile_id.is_some() && previous_profile_id == current_profile_id {
            self.live_readiness.clone()
        } else {
            RuntimeLiveReadiness::default()
        }
    }

    fn apply_stage_result(&mut self, stage: ExecutorStage, result: &ExecutorStageResult) {
        let completed = result.status == "completed";
        if stage == ExecutorStage::ImsRegister {
            self.live_readiness.register_recovery = result.register_recovery;
            if !completed {
                if let (Some(expiry), Some([true, sms_ready, voice_ready])) = (
                    result.register_recovery.and_then(|recovery| recovery.retained_until).filter(|expiry| *expiry > Instant::now()),
                    self.live_readiness.refresh_prior,
                ) {
                    self.live_readiness.ims_registered = true;
                    self.live_readiness.sms_ready = sms_ready;
                    self.live_readiness.voice_ready = voice_ready;
                    self.live_readiness.retained_until = Some(expiry);
                    self.degraded_reason = result.reason.clone();
                    self.live_readiness.normalize_protocol_prerequisites();
                    return;
                }
            }
            self.live_readiness.refresh_prior = None;
            self.live_readiness.retained_until = None;
        }
        match stage {
            ExecutorStage::SimAuth => self.live_readiness.sim_auth_ready = completed,
            ExecutorStage::Epdg => self.live_readiness.epdg_ready = completed,
            ExecutorStage::Ike => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                }
                self.live_readiness.ike_ready = completed;
            }
            ExecutorStage::ChildSa => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                    self.live_readiness.ike_ready = true;
                }
                self.live_readiness.child_sa_ready = completed;
            }
            ExecutorStage::Esp => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                    self.live_readiness.epdg_ready = true;
                    self.live_readiness.ike_ready = true;
                    self.live_readiness.child_sa_ready = true;
                }
                self.live_readiness.esp_ready = completed;
            }
            ExecutorStage::ImsRegister => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                    self.live_readiness.epdg_ready = true;
                    self.live_readiness.ike_ready = true;
                    self.live_readiness.child_sa_ready = true;
                    self.live_readiness.esp_ready = true;
                }
                self.live_readiness.ims_registered = completed;
            }
            ExecutorStage::Sms => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                    self.live_readiness.epdg_ready = true;
                    self.live_readiness.ike_ready = true;
                    self.live_readiness.child_sa_ready = true;
                    self.live_readiness.esp_ready = true;
                    self.live_readiness.ims_registered = true;
                }
                self.live_readiness.sms_ready = completed;
            }
            ExecutorStage::Voice => {
                if completed {
                    self.live_readiness.sim_auth_ready = true;
                    self.live_readiness.epdg_ready = true;
                    self.live_readiness.ike_ready = true;
                    self.live_readiness.child_sa_ready = true;
                    self.live_readiness.esp_ready = true;
                    self.live_readiness.ims_registered = true;
                }
                self.live_readiness.voice_ready = completed;
            }
            ExecutorStage::EsimRestore => {}
        }
        self.live_readiness.normalize_protocol_prerequisites();

        if result.status == "failed" {
            self.degraded_reason = result.reason.clone();
        }
    }

    pub fn readiness(&self) -> VowifiReadiness {
        let mut live_readiness = self.live_readiness.clone();
        live_readiness.normalize_protocol_prerequisites();
        VowifiReadiness {
            identity_ready: self.profile.sim.present,
            profile_matched: self.profile.matched,
            sim_auth_ready: live_readiness.sim_auth_ready,
            epdg_ready: live_readiness.epdg_ready,
            ike_ready: live_readiness.ike_ready,
            child_sa_ready: live_readiness.child_sa_ready,
            esp_ready: live_readiness.esp_ready,
            ims_registered: live_readiness.ims_registered,
            sms_ready: live_readiness.sms_ready,
            voice_ready: live_readiness.voice_ready,
        }
    }

    pub fn status_response(&self) -> VowifiStatusResponse {
        let restore = self.restore.response();
        let readiness = self.readiness();
        let degraded_reason = self
            .degraded_reason
            .clone()
            .or_else(|| restore.degraded_reason.clone());
        let flow = flow::build_runtime_flow(
            &readiness,
            degraded_reason.as_deref(),
            self.phase == RuntimePhase::Failed,
        );
        VowifiStatusResponse {
            phase: flow.stage,
            dataplane_mode: flow.dataplane_mode,
            controlplane_mode: flow.controlplane_mode,
            readiness,
            flow,
            executor: self.executor.clone(),
            profile: self.profile.clone(),
            degraded_reason,
            switch_phase: restore.switch_phase.map(str::to_string),
            switch_token: restore.switch_token,
            phase_ms: restore.phase_ms,
            switch_identity_ready: restore.identity_ready,
            switch_sim_auth_ready: restore.sim_auth_ready,
            switch_retry_count: restore.retry_count,
            diagnostics: self.diagnostics.report(),
            register_recovery: self.live_readiness.register_recovery,
        }
    }
}

#[cfg(test)]
#[path = "runtime_retry_tests.rs"]
mod retry_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::identity::MaskedSimIdentity;

    #[tokio::test]
    async fn live_gate_authorization_does_not_enable_unimplemented_runtime() {
        let runtime = VowifiRuntime::for_line_with_gate(
            "line-test",
            LiveExecutorGateReport {
                live_network_authorized: true,
                device_state_changes_authorized: true,
                adb_path_configured: true,
                device_admin_url_configured: true,
                implementation_ready: false,
                effective_live_network_allowed: false,
                effective_device_state_changes_allowed: false,
                blockers: vec!["live_runtime_executor_not_implemented"],
                sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
            },
        );

        let snapshot = runtime.snapshot().await;
        assert!(snapshot.executor.live_gate.live_network_authorized);
        assert!(snapshot.executor.live_gate.device_state_changes_authorized);
        assert_eq!(snapshot.executor.executor_id, "live_runtime_executor");
        assert_eq!(snapshot.executor.mode, "live_planned");
        assert!(!snapshot.executor.live_network_allowed);
        assert!(!snapshot.executor.device_state_changes_allowed);
        assert!(snapshot
            .executor
            .live_gate
            .blockers
            .contains(&"live_runtime_executor_not_implemented"));
    }

    #[test]
    fn snapshot_status_reflects_profile_match_without_enabling_runtime() {
        let snapshot = RuntimeSnapshot {
            phase: RuntimePhase::ProfileMatched,
            profile: VowifiProfileMatchResponse {
                matched: true,
                matched_prefix: Some("23433".to_string()),
                profile_source: None,
                profile_fallback_reason: None,
                profile: None,
                sim_auth: None,
                epdg: None,
                ike: None,
                dataplane: None,
                ims: None,
                sim: MaskedSimIdentity {
                    present: true,
                    imsi: "***0000".to_string(),
                    ..Default::default()
                },
            },
            executor: NoopRuntimeExecutor.describe(),
            live_readiness: RuntimeLiveReadiness::default(),
            degraded_reason: None,
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        };

        let status = snapshot.status_response();

        assert_eq!(status.phase, "profile_matched");
        assert!(status.readiness.identity_ready);
        assert!(status.readiness.profile_matched);
        assert!(!status.readiness.sim_auth_ready);
        assert_eq!(status.dataplane_mode, "planned");
        assert_eq!(status.flow.steps[2].id, "sim_auth");
        assert_eq!(status.flow.steps[2].state, "ready");
        assert_eq!(status.executor.mode, "disabled_noop");
        assert!(!status.executor.live_network_allowed);
    }

    #[test]
    fn same_profile_refresh_preserves_previous_degraded_reason() {
        let previous = VowifiProfileMatchResponse {
            matched: true,
            matched_prefix: Some("23433".to_string()),
            profile: Some(diagnostics::PublicCarrierProfile::from_profile(
                &super::super::profiles::GB_EE_23433,
            )),
            sim: MaskedSimIdentity {
                present: true,
                imsi: "***0000".to_string(),
                operator_id: "23433".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let current = VowifiProfileMatchResponse {
            matched: true,
            matched_prefix: Some("23433".to_string()),
            profile: Some(diagnostics::PublicCarrierProfile::from_profile(
                &super::super::profiles::GB_EE_23433,
            )),
            sim: MaskedSimIdentity {
                present: true,
                imsi: "***0000".to_string(),
                operator_id: "23433".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let changed = VowifiProfileMatchResponse::default();

        assert!(same_matched_profile(&previous, &current));
        assert!(!same_matched_profile(&previous, &changed));
    }

    #[test]
    fn identity_loss_preserves_live_executor_gate() {
        let snapshot = RuntimeSnapshot {
            phase: RuntimePhase::ProfileMatched,
            profile: VowifiProfileMatchResponse {
                matched: true,
                matched_prefix: Some("23433".to_string()),
                profile: Some(diagnostics::PublicCarrierProfile::from_profile(
                    &super::super::profiles::GB_EE_23433,
                )),
                sim: MaskedSimIdentity {
                    present: true,
                    imsi: "***0000".to_string(),
                    operator_id: "23433".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            executor: LiveRuntimeExecutor::from_gate(LiveExecutorGateReport {
                live_network_authorized: true,
                device_state_changes_authorized: true,
                adb_path_configured: true,
                device_admin_url_configured: true,
                implementation_ready: true,
                effective_live_network_allowed: true,
                effective_device_state_changes_allowed: true,
                blockers: vec![],
                sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
            })
            .describe(),
            live_readiness: RuntimeLiveReadiness {
                epdg_ready: true,
                ike_ready: true,
                ..Default::default()
            },
            degraded_reason: Some("transient_modem_unavailable".to_string()),
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        };

        let reset = reset_identity_preserving_runtime(snapshot);

        assert_eq!(reset.phase, RuntimePhase::ProfileMatched);
        assert!(reset.profile.matched);
        assert!(reset.profile.sim.present);
        assert_eq!(reset.executor.executor_id, "live_runtime_executor");
        assert_eq!(reset.executor.mode, "live");
        assert!(reset.executor.live_network_allowed);
        assert!(reset.live_readiness.epdg_ready);
        assert_eq!(
            reset.degraded_reason.as_deref(),
            Some("transient_modem_unavailable")
        );
    }

    #[test]
    fn stage_result_updates_epdg_readiness_without_promoting_later_stages() {
        let mut snapshot = RuntimeSnapshot {
            phase: RuntimePhase::ProfileMatched,
            profile: VowifiProfileMatchResponse {
                matched: true,
                matched_prefix: Some("23433".to_string()),
                profile_source: None,
                profile_fallback_reason: None,
                profile: None,
                sim_auth: None,
                epdg: None,
                ike: None,
                dataplane: None,
                ims: None,
                sim: MaskedSimIdentity {
                    present: true,
                    imsi: "***0000".to_string(),
                    ..Default::default()
                },
            },
            executor: NoopRuntimeExecutor.describe(),
            live_readiness: RuntimeLiveReadiness::default(),
            degraded_reason: None,
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        };

        snapshot.apply_stage_result(
            ExecutorStage::Epdg,
            &ExecutorStageResult {
                stage: "epdg",
                status: "completed",
                readiness_key: "epdg_ready",
                reason: None,
                soak_observation: None,
                register_recovery: None,
            },
        );

        let status = snapshot.status_response();

        assert!(status.readiness.sim_auth_ready);
        assert!(status.readiness.epdg_ready);
        assert!(!status.readiness.ike_ready);
        assert!(!status.readiness.sms_ready);
        assert_eq!(status.flow.steps[2].state, "done");
        assert_eq!(status.phase, "epdg_ready");
    }

    #[test]
    fn live_refresh_advances_tunnel_layers_in_protocol_order() {
        assert_eq!(
            live_refresh_stages(&RuntimeLiveReadiness::default()),
            vec![ExecutorStage::SimAuth]
        );
        assert_eq!(
            live_refresh_stages_for(
                &RuntimeLiveReadiness::default(),
                LiveRefreshScope::StatusProbe
            ),
            vec![ExecutorStage::Epdg]
        );
        let readiness = RuntimeLiveReadiness {
            sim_auth_ready: true,
            epdg_ready: true,
            ..Default::default()
        };
        assert_eq!(live_refresh_stages(&readiness), vec![ExecutorStage::Esp]);
        assert!(live_refresh_stages_for(&readiness, LiveRefreshScope::StatusProbe).is_empty());
        assert!(live_refresh_stages_for(
            &RuntimeLiveReadiness {
                epdg_ready: true,
                ike_ready: true,
                ..Default::default()
            },
            LiveRefreshScope::StatusProbe
        )
        .is_empty());
        assert_eq!(
            live_refresh_stages(&RuntimeLiveReadiness {
                sim_auth_ready: true,
                epdg_ready: true,
                ike_ready: true,
                ..Default::default()
            }),
            vec![ExecutorStage::Esp]
        );
        assert_eq!(
            live_refresh_stages(&RuntimeLiveReadiness {
                sim_auth_ready: true,
                epdg_ready: true,
                ike_ready: true,
                child_sa_ready: true,
                ..Default::default()
            }),
            vec![ExecutorStage::Esp]
        );

        let mut snapshot = RuntimeSnapshot {
            phase: RuntimePhase::ProfileMatched,
            profile: VowifiProfileMatchResponse {
                matched: true,
                matched_prefix: Some("23433".to_string()),
                profile_source: None,
                profile_fallback_reason: None,
                profile: None,
                sim_auth: None,
                epdg: None,
                ike: None,
                dataplane: None,
                ims: None,
                sim: MaskedSimIdentity {
                    present: true,
                    imsi: "***0000".to_string(),
                    ..Default::default()
                },
            },
            executor: NoopRuntimeExecutor.describe(),
            live_readiness: RuntimeLiveReadiness {
                epdg_ready: true,
                ..Default::default()
            },
            degraded_reason: Some("previous_failure".to_string()),
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        };

        snapshot.apply_stage_result(
            ExecutorStage::Esp,
            &ExecutorStageResult {
                stage: "esp",
                status: "completed",
                readiness_key: "esp_ready",
                reason: None,
                soak_observation: None,
                register_recovery: None,
            },
        );

        assert!(snapshot.live_readiness.ike_ready);
        assert!(snapshot.live_readiness.sim_auth_ready);
        assert!(snapshot.live_readiness.epdg_ready);
        assert!(snapshot.live_readiness.child_sa_ready);
        assert!(snapshot.live_readiness.esp_ready);
        assert_eq!(
            snapshot.degraded_reason.as_deref(),
            Some("previous_failure")
        );
    }

    #[test]
    fn status_response_normalizes_protocol_prerequisites() {
        let snapshot = RuntimeSnapshot {
            phase: RuntimePhase::ProfileMatched,
            profile: VowifiProfileMatchResponse {
                matched: true,
                matched_prefix: Some("23433".to_string()),
                profile_source: None,
                profile_fallback_reason: None,
                profile: None,
                sim_auth: None,
                epdg: None,
                ike: None,
                dataplane: None,
                ims: None,
                sim: MaskedSimIdentity {
                    present: true,
                    imsi: "***0000".to_string(),
                    ..Default::default()
                },
            },
            executor: NoopRuntimeExecutor.describe(),
            live_readiness: RuntimeLiveReadiness {
                sms_ready: true,
                ..Default::default()
            },
            degraded_reason: None,
            restore: RestoreProgress::default(),
            diagnostics: Default::default(),
        };

        let readiness = snapshot.status_response().readiness;

        assert!(readiness.sim_auth_ready);
        assert!(readiness.epdg_ready);
        assert!(readiness.ike_ready);
        assert!(readiness.child_sa_ready);
        assert!(readiness.esp_ready);
        assert!(readiness.ims_registered);
        assert!(readiness.sms_ready);
    }
}
