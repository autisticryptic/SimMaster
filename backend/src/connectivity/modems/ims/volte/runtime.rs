//! VoLTE runtime: stage/phase state machine + status snapshot.
//!
//! Clean-room note: the `stage`/`phase`/`registration_mode` string values and
//! the control-response field names are a hard interoperability contract with
//! the published frontend `volteStatus.js`. They are reproduced verbatim so the
//! existing UI renders. The implementation itself is written from 3GPP/RFC
//! specifications, not derived from any third-party binary source.
//!
//! Like `VowifiRuntime`, this is a passive, cloneable state container driven by
//! external callers (HTTP handlers, the auto-restore task). Progress happens
//! when a driver advances the stage; teardown is `reset_runtime`, which bumps a
//! generation counter to cancel any in-flight advance.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

/// Connection sub-stage. String values MUST match `volteStatus.js` `b()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolteStage {
    Disabled,
    Starting,
    Identity,
    IdentityAka,
    Radio,
    ImsContext,
    Pcscf,
    Ipv6Preflight,
    Modem,
    Bearer,
    BearerDual,
    BearerIpv4,
    BearerIpv6,
    IpConfig,
    RegisterInitial,
    Ipsec,
    RegisterAuthenticated,
    RegisterRefresh,
    RegisterIpsec,
    RegisterUdp,
    Registered,
    Stopping,
}

impl VolteStage {
    pub fn as_str(self) -> &'static str {
        match self {
            VolteStage::Disabled => "disabled",
            VolteStage::Starting => "starting",
            VolteStage::Identity => "identity",
            VolteStage::IdentityAka => "identity_aka",
            VolteStage::Radio => "radio",
            VolteStage::ImsContext => "ims_context",
            VolteStage::Pcscf => "pcscf",
            VolteStage::Ipv6Preflight => "ipv6_preflight",
            VolteStage::Modem => "modem",
            VolteStage::Bearer => "bearer",
            VolteStage::BearerDual => "bearer_dual",
            VolteStage::BearerIpv4 => "bearer_ipv4",
            VolteStage::BearerIpv6 => "bearer_ipv6",
            VolteStage::IpConfig => "ip_config",
            VolteStage::RegisterInitial => "register_initial",
            VolteStage::Ipsec => "ipsec",
            VolteStage::RegisterAuthenticated => "register_authenticated",
            VolteStage::RegisterRefresh => "register_refresh",
            VolteStage::RegisterIpsec => "register_ipsec",
            VolteStage::RegisterUdp => "register_udp",
            VolteStage::Registered => "registered",
            VolteStage::Stopping => "stopping",
        }
    }
}

const MAX_CONNECTION_ATTEMPTS: usize = 32;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VolteConnectionAttempt {
    pub sequence: u32,
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_family: Option<String>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Structured context captured from the runtime snapshot at the instant this
    /// attempt was recorded, so the Web UI can show exactly "where and with what"
    /// a step failed without parsing the free-text `detail`. Each is skipped when
    /// not yet known at that point in the connect flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_cid: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qmi_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcscf: Option<String>,
    pub at: String,
}

/// High-level phase. String values MUST match `volteStatus.js` `g()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoltePhase {
    Disabled,
    Starting,
    Registered,
    Degraded,
    Stopping,
}

/// Recovery workflow state exposed to the Web UI.  Registration stages remain
/// focused on IMS itself; this state explains why a requested connection is
/// waiting, retrying, or deliberately stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolteRecoveryState {
    Idle,
    WaitingModem,
    RestartingBaseband,
    Connecting,
    Registered,
    Exhausted,
}

impl VolteRecoveryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::WaitingModem => "waiting_modem",
            Self::RestartingBaseband => "restarting_baseband",
            Self::Connecting => "connecting",
            Self::Registered => "registered",
            Self::Exhausted => "exhausted",
        }
    }
}

impl VoltePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            VoltePhase::Disabled => "disabled",
            VoltePhase::Starting => "starting",
            VoltePhase::Registered => "registered",
            VoltePhase::Degraded => "degraded",
            VoltePhase::Stopping => "stopping",
        }
    }
}

/// Registration transport mode. String values MUST match `volteStatus.js` `v()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationMode {
    None,
    Ipsec,
    Udp,
}

impl RegistrationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrationMode::None => "",
            RegistrationMode::Ipsec => "ipsec",
            RegistrationMode::Udp => "udp",
        }
    }
}

/// Canonical runtime snapshot. Field names + wire format match the frontend
/// `control` response contract (see `VolteControlResponse` in models).
#[derive(Debug, Clone)]
pub struct VolteSnapshot {
    pub phase: VoltePhase,
    pub stage: VolteStage,
    pub registration_mode: RegistrationMode,
    pub pcscf: Option<String>,
    pub session_started_at: Option<String>,
    pub registered_at: Option<String>,
    pub last_register_refresh_at: Option<String>,
    pub last_rx_at: Option<String>,
    pub last_tx_at: Option<String>,
    pub last_error: Option<String>,
    pub last_failure_at: Option<String>,
    pub next_retry_at: Option<String>,
    pub sent_count: u64,
    pub received_count: u64,
    pub duplicate_count: u64,
    pub reconnect_count: u64,
    pub register_refresh_count: u64,
    pub data_path_mode: Option<String>,
    /// Primary QMI control port for this line — what ModemManager uses for normal
    /// mobile data.
    pub qmi_device: Option<String>,
    /// Dedicated QMI endpoint carrying this line's IMS/VoLTE session, when one is
    /// prepared. Always belongs to the same baseband as `qmi_device` (paired by
    /// sysfs ancestor), so multi-baseband hosts never cross wires.
    pub secondary_qmi_device: Option<String>,
    /// rpmsg channel backing `secondary_qmi_device`, e.g. `DATA6_CNTL`.
    pub secondary_qmi_channel: Option<String>,
    pub bearer_interface: Option<String>,
    pub bearer_ip_type: Option<String>,
    pub bearer_path: Option<String>,
    pub at_cid: Option<u8>,
    pub current_ip_family: Option<String>,
    pub identity_source: Option<String>,
    pub usim_aid: Option<String>,
    pub isim_aid: Option<String>,
    pub connection_attempts: Vec<VolteConnectionAttempt>,
    pub recovery_state: VolteRecoveryState,
    pub recovery_source: Option<String>,
    pub retry_attempt: u32,
    pub retry_max: u32,
    pub modem_restart_attempt: u32,
    pub modem_restart_max: u32,
    pub manual_retry_available: bool,
}

impl Default for VolteSnapshot {
    fn default() -> Self {
        Self {
            phase: VoltePhase::Disabled,
            stage: VolteStage::Disabled,
            registration_mode: RegistrationMode::None,
            pcscf: None,
            session_started_at: None,
            registered_at: None,
            last_register_refresh_at: None,
            last_rx_at: None,
            last_tx_at: None,
            last_error: None,
            last_failure_at: None,
            next_retry_at: None,
            sent_count: 0,
            received_count: 0,
            duplicate_count: 0,
            reconnect_count: 0,
            register_refresh_count: 0,
            data_path_mode: None,
            qmi_device: None,
            secondary_qmi_device: None,
            secondary_qmi_channel: None,
            at_cid: None,
            bearer_interface: None,
            bearer_path: None,
            bearer_ip_type: None,
            current_ip_family: None,
            identity_source: None,
            usim_aid: None,
            isim_aid: None,
            connection_attempts: Vec::new(),
            recovery_state: VolteRecoveryState::Idle,
            recovery_source: None,
            retry_attempt: 0,
            retry_max: 5,
            modem_restart_attempt: 0,
            modem_restart_max: 3,
            manual_retry_available: false,
        }
    }
}

impl VolteSnapshot {
    pub fn registered(&self) -> bool {
        self.phase == VoltePhase::Registered
    }
}

/// Serializable per-line runtime projection returned by the VoLTE line APIs.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VolteRuntimeStatus {
    pub phase: String,
    pub stage: String,
    pub registration_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcscf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_register_refresh_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rx_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tx_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    pub registered: bool,
    pub sent_count: u64,
    pub received_count: u64,
    pub duplicate_count: u64,
    pub reconnect_count: u64,
    pub register_refresh_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_path_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qmi_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_qmi_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_qmi_channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_ip_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_ip_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usim_aid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isim_aid: Option<String>,
    pub connection_attempts: Vec<VolteConnectionAttempt>,
    pub recovery_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_source: Option<String>,
    pub retry_attempt: u32,
    pub retry_max: u32,
    pub modem_restart_attempt: u32,
    pub modem_restart_max: u32,
    pub manual_retry_available: bool,
}

impl From<&VolteSnapshot> for VolteRuntimeStatus {
    fn from(s: &VolteSnapshot) -> Self {
        Self {
            phase: s.phase.as_str().to_string(),
            stage: s.stage.as_str().to_string(),
            registration_mode: s.registration_mode.as_str().to_string(),
            pcscf: s.pcscf.clone(),
            session_started_at: s.session_started_at.clone(),
            registered_at: s.registered_at.clone(),
            last_register_refresh_at: s.last_register_refresh_at.clone(),
            last_rx_at: s.last_rx_at.clone(),
            last_tx_at: s.last_tx_at.clone(),
            last_error: s.last_error.clone(),
            last_failure_at: s.last_failure_at.clone(),
            next_retry_at: s.next_retry_at.clone(),
            registered: s.registered(),
            sent_count: s.sent_count,
            received_count: s.received_count,
            duplicate_count: s.duplicate_count,
            reconnect_count: s.reconnect_count,
            register_refresh_count: s.register_refresh_count,
            data_path_mode: s.data_path_mode.clone(),
            qmi_device: s.qmi_device.clone(),
            secondary_qmi_device: s.secondary_qmi_device.clone(),
            secondary_qmi_channel: s.secondary_qmi_channel.clone(),
            bearer_interface: s.bearer_interface.clone(),
            bearer_ip_type: s.bearer_ip_type.clone(),
            current_ip_family: s.current_ip_family.clone(),
            identity_source: s.identity_source.clone(),
            usim_aid: s.usim_aid.clone(),
            isim_aid: s.isim_aid.clone(),
            connection_attempts: s.connection_attempts.clone(),
            recovery_state: s.recovery_state.as_str().to_string(),
            recovery_source: s.recovery_source.clone(),
            retry_attempt: s.retry_attempt,
            retry_max: s.retry_max,
            modem_restart_attempt: s.modem_restart_attempt,
            modem_restart_max: s.modem_restart_max,
            manual_retry_available: s.manual_retry_available,
        }
    }
}

/// Cloneable runtime handle. Mirrors the `VowifiRuntime` shape:
/// single source of truth + a serialization mutex + a generation counter that
/// acts as a cancellation token for in-flight advances.
#[derive(Clone)]
pub struct VolteRuntime {
    snapshot: Arc<RwLock<VolteSnapshot>>,
    advance_lock: Arc<Mutex<()>>,
    generation: Arc<AtomicU64>,
}

impl Default for VolteRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl VolteRuntime {
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(VolteSnapshot::default())),
            advance_lock: Arc::new(Mutex::new(())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Read-lock clone of the current snapshot.
    pub async fn snapshot(&self) -> VolteSnapshot {
        self.snapshot.read().await.clone()
    }

    /// Serializable projection for the API.
    pub async fn status(&self) -> VolteRuntimeStatus {
        VolteRuntimeStatus::from(&*self.snapshot.read().await)
    }

    /// Current generation token; a driver captures this before a long advance
    /// and re-checks it to detect a concurrent `reset_runtime`.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Apply a mutation to the snapshot under the write lock.
    pub async fn update(&self, f: impl FnOnce(&mut VolteSnapshot)) -> VolteSnapshot {
        let mut guard = self.snapshot.write().await;
        f(&mut guard);
        guard.clone()
    }

    pub async fn record_attempt(
        &self,
        stage: VolteStage,
        ip_family: Option<&str>,
        outcome: &str,
        error: Option<&crate::connectivity::modems::ims::volte::errors::VolteError>,
        detail: Option<String>,
    ) {
        self.update(|snapshot| {
            let sequence = snapshot
                .connection_attempts
                .last()
                .map_or(1, |attempt| attempt.sequence.saturating_add(1));
            // Auto-capture the structured context that the snapshot already
            // tracks at record time, so each attempt row carries the AT CID,
            // QMI device, bearer path/interface and P-CSCF as first-class
            // fields instead of smuggling them inside the free-text `detail`.
            let at_cid = snapshot.at_cid;
            let qmi_device = snapshot.qmi_device.clone();
            let bearer_path = snapshot.bearer_path.clone();
            let bearer_interface = snapshot.bearer_interface.clone();
            let pcscf = snapshot.pcscf.clone();
            snapshot.connection_attempts.push(VolteConnectionAttempt {
                sequence,
                stage: stage.as_str().to_string(),
                ip_family: ip_family.map(str::to_string),
                outcome: outcome.to_string(),
                error_code: error.map(|error| error.code().to_string()),
                detail: detail
                    .or_else(|| error.and_then(|error| error.detail().map(str::to_string))),
                at_cid,
                qmi_device,
                bearer_path,
                interface: bearer_interface,
                pcscf,
                at: chrono::Utc::now().to_rfc3339(),
            });
            if snapshot.connection_attempts.len() > MAX_CONNECTION_ATTEMPTS {
                let excess = snapshot.connection_attempts.len() - MAX_CONNECTION_ATTEMPTS;
                snapshot.connection_attempts.drain(..excess);
            }
        })
        .await;
    }

    /// Acquire the advance serialization lock. Drivers hold this for the
    /// duration of a stage-progression pass so two passes don't interleave.
    pub async fn advance_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.advance_lock.lock().await
    }

    /// Teardown / cancel: bump generation (invalidating in-flight advances) and
    /// reset the snapshot to a disabled/degraded baseline.
    pub async fn reset_runtime(&self, reason: impl Into<String>) -> VolteSnapshot {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let reason = reason.into();
        self.update(|s| {
            let prev_reconnect = s.reconnect_count;
            let prev_refresh = s.register_refresh_count;
            *s = VolteSnapshot {
                phase: VoltePhase::Disabled,
                stage: VolteStage::Disabled,
                reconnect_count: prev_reconnect,
                register_refresh_count: prev_refresh,
                last_error: if reason.is_empty() {
                    None
                } else {
                    Some(reason)
                },
                ..VolteSnapshot::default()
            };
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_strings_match_frontend_contract() {
        // Exact set from volteStatus.js b().
        assert_eq!(VolteStage::Disabled.as_str(), "disabled");
        assert_eq!(VolteStage::Starting.as_str(), "starting");
        assert_eq!(VolteStage::Identity.as_str(), "identity");
        assert_eq!(VolteStage::IdentityAka.as_str(), "identity_aka");
        assert_eq!(VolteStage::Radio.as_str(), "radio");
        assert_eq!(VolteStage::Pcscf.as_str(), "pcscf");
        assert_eq!(VolteStage::Modem.as_str(), "modem");
        assert_eq!(VolteStage::Bearer.as_str(), "bearer");
        assert_eq!(VolteStage::RegisterIpsec.as_str(), "register_ipsec");
        assert_eq!(VolteStage::RegisterUdp.as_str(), "register_udp");
        assert_eq!(VolteStage::Registered.as_str(), "registered");
        assert_eq!(VolteStage::Stopping.as_str(), "stopping");
    }

    #[test]
    fn phase_strings_match_frontend_contract() {
        assert_eq!(VoltePhase::Disabled.as_str(), "disabled");
        assert_eq!(VoltePhase::Starting.as_str(), "starting");
        assert_eq!(VoltePhase::Registered.as_str(), "registered");
        assert_eq!(VoltePhase::Degraded.as_str(), "degraded");
        assert_eq!(VoltePhase::Stopping.as_str(), "stopping");
    }

    #[test]
    fn registration_mode_strings_match_frontend_contract() {
        assert_eq!(RegistrationMode::None.as_str(), "");
        assert_eq!(RegistrationMode::Ipsec.as_str(), "ipsec");
        assert_eq!(RegistrationMode::Udp.as_str(), "udp");
    }

    #[test]
    fn recovery_state_strings_are_stable() {
        assert_eq!(VolteRecoveryState::Idle.as_str(), "idle");
        assert_eq!(VolteRecoveryState::WaitingModem.as_str(), "waiting_modem");
        assert_eq!(
            VolteRecoveryState::RestartingBaseband.as_str(),
            "restarting_baseband"
        );
        assert_eq!(VolteRecoveryState::Connecting.as_str(), "connecting");
        assert_eq!(VolteRecoveryState::Registered.as_str(), "registered");
        assert_eq!(VolteRecoveryState::Exhausted.as_str(), "exhausted");
    }

    #[test]
    fn default_snapshot_is_disabled_and_unregistered() {
        let s = VolteSnapshot::default();
        assert_eq!(s.phase, VoltePhase::Disabled);
        assert_eq!(s.stage, VolteStage::Disabled);
        assert!(!s.registered());
        let status = VolteRuntimeStatus::from(&s);
        assert_eq!(status.phase, "disabled");
        assert_eq!(status.stage, "disabled");
        assert_eq!(status.registration_mode, "");
        assert!(!status.registered);
        assert_eq!(status.recovery_state, "idle");
        assert_eq!(status.retry_max, 5);
        assert_eq!(status.modem_restart_max, 3);
    }

    #[tokio::test]
    async fn reset_runtime_bumps_generation_and_disables() {
        let rt = VolteRuntime::new();
        let g0 = rt.generation();
        rt.update(|s| {
            s.phase = VoltePhase::Registered;
            s.stage = VolteStage::Registered;
            s.reconnect_count = 5;
        })
        .await;
        let snap = rt.reset_runtime("volte_disabled").await;
        assert_eq!(snap.phase, VoltePhase::Disabled);
        assert_eq!(snap.stage, VolteStage::Disabled);
        assert_eq!(
            snap.reconnect_count, 5,
            "reconnect count is preserved across reset"
        );
        assert_eq!(snap.last_error.as_deref(), Some("volte_disabled"));
        assert_eq!(rt.generation(), g0 + 1);
    }
}
