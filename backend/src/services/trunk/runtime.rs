//! Per-line Trunk runtime state shared by the API and the active SIP driver.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::Serialize;
use tokio::{
    sync::{watch, Mutex, RwLock},
    task::JoinHandle,
};

use crate::{
    platform::config::{TrunkProfileConfig, TrunkRegistrationMode},
    services::trunk::operator::OperatorLink,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrunkPhase {
    Disabled,
    Configured,
    Starting,
    Ready,
    Registered,
    Degraded,
    Stopping,
}

impl TrunkPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Configured => "configured",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Registered => "registered",
            Self::Degraded => "degraded",
            Self::Stopping => "stopping",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrunkStage {
    Disabled,
    Configured,
    Resolving,
    Connecting,
    Registering,
    Listening,
    Registered,
    Backoff,
    Stopping,
}

impl TrunkStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Configured => "configured",
            Self::Resolving => "resolving",
            Self::Connecting => "connecting",
            Self::Registering => "registering",
            Self::Listening => "listening",
            Self::Registered => "registered",
            Self::Backoff => "backoff",
            Self::Stopping => "stopping",
        }
    }
}

fn mode_name(mode: TrunkRegistrationMode) -> &'static str {
    match mode {
        TrunkRegistrationMode::StaticPeer => "static_peer",
        TrunkRegistrationMode::OutboundRegister => "outbound_register",
    }
}

#[derive(Debug, Clone)]
pub struct TrunkSnapshot {
    pub phase: TrunkPhase,
    pub stage: TrunkStage,
    pub enabled: bool,
    pub registration_mode: TrunkRegistrationMode,
    pub peer: Option<String>,
    pub local_endpoint: Option<String>,
    pub registered: bool,
    pub last_sip_status: Option<u16>,
    pub started_at: Option<String>,
    pub registered_at: Option<String>,
    pub expires_at: Option<String>,
    pub next_retry_at: Option<String>,
    pub last_error: Option<String>,
    pub register_attempts: u64,
    pub reconnect_count: u64,
    pub active_dialogs: u64,
    pub active_calls: u64,
    pub sip_rx_frames: u64,
    pub sip_rx_bytes: u64,
    pub sip_tx_frames: u64,
    pub sip_tx_bytes: u64,
    pub invite_count: u64,
    pub reinvite_count: u64,
    pub media_negotiations: u64,
    pub video_negotiations: u64,
    pub last_activity_at: Option<String>,
}

impl Default for TrunkSnapshot {
    fn default() -> Self {
        Self {
            phase: TrunkPhase::Disabled,
            stage: TrunkStage::Disabled,
            enabled: false,
            registration_mode: TrunkRegistrationMode::StaticPeer,
            peer: None,
            local_endpoint: None,
            registered: false,
            last_sip_status: None,
            started_at: None,
            registered_at: None,
            expires_at: None,
            next_retry_at: None,
            last_error: None,
            register_attempts: 0,
            reconnect_count: 0,
            active_dialogs: 0,
            active_calls: 0,
            sip_rx_frames: 0,
            sip_rx_bytes: 0,
            sip_tx_frames: 0,
            sip_tx_bytes: 0,
            invite_count: 0,
            reinvite_count: 0,
            media_negotiations: 0,
            video_negotiations: 0,
            last_activity_at: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TrunkRuntimeStatus {
    pub phase: String,
    pub stage: String,
    pub enabled: bool,
    pub registration_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<String>,
    pub registered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sip_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub register_attempts: u64,
    pub reconnect_count: u64,
    pub active_dialogs: u64,
    pub active_calls: u64,
    pub sip_rx_frames: u64,
    pub sip_rx_bytes: u64,
    pub sip_tx_frames: u64,
    pub sip_tx_bytes: u64,
    pub invite_count: u64,
    pub reinvite_count: u64,
    pub media_negotiations: u64,
    pub video_negotiations: u64,
    pub operator_commands: u64,
    pub operator_events: u64,
    pub dtmf_events: u64,
    pub active_media_relays: u64,
    pub rtp_from_asterisk_packets: u64,
    pub rtp_from_asterisk_bytes: u64,
    pub rtp_to_asterisk_packets: u64,
    pub rtp_to_asterisk_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
}

impl From<&TrunkSnapshot> for TrunkRuntimeStatus {
    fn from(snapshot: &TrunkSnapshot) -> Self {
        Self {
            phase: snapshot.phase.as_str().to_string(),
            stage: snapshot.stage.as_str().to_string(),
            enabled: snapshot.enabled,
            registration_mode: mode_name(snapshot.registration_mode).to_string(),
            peer: snapshot.peer.clone(),
            local_endpoint: snapshot.local_endpoint.clone(),
            registered: snapshot.registered,
            last_sip_status: snapshot.last_sip_status,
            started_at: snapshot.started_at.clone(),
            registered_at: snapshot.registered_at.clone(),
            expires_at: snapshot.expires_at.clone(),
            next_retry_at: snapshot.next_retry_at.clone(),
            last_error: snapshot.last_error.clone(),
            register_attempts: snapshot.register_attempts,
            reconnect_count: snapshot.reconnect_count,
            active_dialogs: snapshot.active_dialogs,
            active_calls: snapshot.active_calls,
            sip_rx_frames: snapshot.sip_rx_frames,
            sip_rx_bytes: snapshot.sip_rx_bytes,
            sip_tx_frames: snapshot.sip_tx_frames,
            sip_tx_bytes: snapshot.sip_tx_bytes,
            invite_count: snapshot.invite_count,
            reinvite_count: snapshot.reinvite_count,
            media_negotiations: snapshot.media_negotiations,
            video_negotiations: snapshot.video_negotiations,
            operator_commands: 0,
            operator_events: 0,
            dtmf_events: 0,
            active_media_relays: 0,
            rtp_from_asterisk_packets: 0,
            rtp_from_asterisk_bytes: 0,
            rtp_to_asterisk_packets: 0,
            rtp_to_asterisk_bytes: 0,
            last_activity_at: snapshot.last_activity_at.clone(),
        }
    }
}

#[derive(Clone)]
pub struct TrunkRuntime {
    snapshot: Arc<RwLock<TrunkSnapshot>>,
    active_profile: Arc<RwLock<Option<TrunkProfileConfig>>>,
    operation_lock: Arc<Mutex<()>>,
    driver_task: Arc<Mutex<Option<TrunkDriverTask>>>,
    generation: Arc<AtomicU64>,
    operator: OperatorLink,
}

impl Default for TrunkRuntime {
    fn default() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(TrunkSnapshot::default())),
            active_profile: Arc::new(RwLock::new(None)),
            operation_lock: Arc::new(Mutex::new(())),
            driver_task: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            operator: OperatorLink::default(),
        }
    }
}

struct TrunkDriverTask {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl TrunkRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_operator(operator: OperatorLink) -> Self {
        Self {
            operator,
            ..Self::default()
        }
    }

    pub async fn snapshot(&self) -> TrunkSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn status(&self) -> TrunkRuntimeStatus {
        let mut status = TrunkRuntimeStatus::from(&*self.snapshot.read().await);
        let diagnostics = self.operator.diagnostics();
        status.operator_commands = diagnostics.command_count;
        status.operator_events = diagnostics.event_count;
        status.dtmf_events = diagnostics.dtmf_events;
        status.active_media_relays = diagnostics.active_relays;
        status.rtp_from_asterisk_packets = diagnostics.rtp_from_asterisk_packets;
        status.rtp_from_asterisk_bytes = diagnostics.rtp_from_asterisk_bytes;
        status.rtp_to_asterisk_packets = diagnostics.rtp_to_asterisk_packets;
        status.rtp_to_asterisk_bytes = diagnostics.rtp_to_asterisk_bytes;
        status
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub async fn operation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.operation_lock.lock().await
    }

    pub(crate) fn state_writer(&self, generation: u64) -> TrunkStateWriter {
        TrunkStateWriter {
            snapshot: Arc::clone(&self.snapshot),
            current_generation: Arc::clone(&self.generation),
            generation,
        }
    }

    /// Apply persisted intent and cancel any future driver using the previous
    /// generation. Enabling stops at `configured` until the D4 driver starts.
    pub async fn apply_profile(&self, profile: &TrunkProfileConfig) -> TrunkSnapshot {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.active_profile.write().await = Some(profile.clone());
        let mut snapshot = self.snapshot.write().await;
        let reconnect_count = snapshot.reconnect_count;
        *snapshot = if profile.enabled {
            TrunkSnapshot {
                phase: TrunkPhase::Configured,
                stage: TrunkStage::Configured,
                enabled: true,
                registration_mode: profile.registration_mode,
                peer: Some(format!(
                    "{}:{}",
                    profile.asterisk_host, profile.asterisk_port
                )),
                local_endpoint: None,
                reconnect_count,
                ..TrunkSnapshot::default()
            }
        } else {
            TrunkSnapshot {
                registration_mode: profile.registration_mode,
                peer: if profile.asterisk_host.trim().is_empty() {
                    None
                } else {
                    Some(format!(
                        "{}:{}",
                        profile.asterisk_host, profile.asterisk_port
                    ))
                },
                reconnect_count,
                ..TrunkSnapshot::default()
            }
        };
        snapshot.clone()
    }

    /// Stop the previous per-line endpoint, apply the persisted profile and,
    /// when enabled, start a fresh D4 driver. A registered driver gets a short
    /// grace period to send Expires: 0 before it is force-aborted.
    pub async fn activate_profile(&self, profile: &TrunkProfileConfig) -> TrunkSnapshot {
        let _guard = self.operation_guard().await;
        if let Some(driver) = self.driver_task.lock().await.take() {
            let _ = driver.shutdown.send(true);
            let mut task = driver.task;
            if tokio::time::timeout(std::time::Duration::from_secs(6), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        let snapshot = self.apply_profile(profile).await;
        if profile.enabled {
            let generation = self.generation();
            let state = self.state_writer(generation);
            let profile = profile.clone();
            let operator = self.operator.clone();
            let (shutdown, shutdown_rx) = watch::channel(false);
            let task = tokio::spawn(async move {
                crate::services::trunk::driver::run(profile, state, shutdown_rx, operator).await;
            });
            *self.driver_task.lock().await = Some(TrunkDriverTask { shutdown, task });
        }
        snapshot
    }

    /// Startup/hotplug reconciliation that does not disturb an already active
    /// D4 session. Explicit config changes continue to use `apply_profile`.
    pub async fn reconcile_profile(&self, profile: &TrunkProfileConfig) -> TrunkSnapshot {
        let current = self.active_profile.read().await.clone();
        if current.as_ref() != Some(profile) {
            return self.activate_profile(profile).await;
        }
        self.snapshot().await
    }
}

/// Cloneable, task-safe access to one generation of runtime state. Every write
/// is discarded after a profile change or explicit disable.
#[derive(Clone)]
pub(crate) struct TrunkStateWriter {
    snapshot: Arc<RwLock<TrunkSnapshot>>,
    current_generation: Arc<AtomicU64>,
    generation: u64,
}

impl TrunkStateWriter {
    pub fn is_current(&self) -> bool {
        self.current_generation.load(Ordering::SeqCst) == self.generation
    }

    pub async fn update(&self, update: impl FnOnce(&mut TrunkSnapshot)) -> bool {
        if !self.is_current() {
            return false;
        }
        let mut snapshot = self.snapshot.write().await;
        if !self.is_current() {
            return false;
        }
        update(&mut snapshot);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enabled_profile_stops_at_configured_before_driver_exists() {
        let runtime = TrunkRuntime::new();
        let profile = TrunkProfileConfig {
            enabled: true,
            registration_mode: TrunkRegistrationMode::OutboundRegister,
            asterisk_host: "pbx.example.com".to_string(),
            asterisk_port: 5060,
            ..TrunkProfileConfig::default()
        };
        runtime.apply_profile(&profile).await;
        let status = runtime.status().await;
        assert_eq!(status.phase, "configured");
        assert_eq!(status.stage, "configured");
        assert_eq!(status.registration_mode, "outbound_register");
        assert_eq!(status.peer.as_deref(), Some("pbx.example.com:5060"));
        assert!(!status.registered);
    }

    #[tokio::test]
    async fn disabling_profile_cancels_previous_generation() {
        let runtime = TrunkRuntime::new();
        let generation = runtime.generation();
        runtime.apply_profile(&TrunkProfileConfig::default()).await;
        assert_eq!(runtime.generation(), generation + 1);
        assert_eq!(runtime.status().await.phase, "disabled");
    }
}
