//! Per-line Trunk runtime state shared by the API and the future SIP driver.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

use crate::infra::config::{TrunkProfileConfig, TrunkRegistrationMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrunkPhase {
    Disabled,
    Configured,
    Starting,
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
    pub registered: bool,
    pub last_sip_status: Option<u16>,
    pub started_at: Option<String>,
    pub registered_at: Option<String>,
    pub expires_at: Option<String>,
    pub next_retry_at: Option<String>,
    pub last_error: Option<String>,
    pub register_attempts: u64,
    pub reconnect_count: u64,
}

impl Default for TrunkSnapshot {
    fn default() -> Self {
        Self {
            phase: TrunkPhase::Disabled,
            stage: TrunkStage::Disabled,
            enabled: false,
            registration_mode: TrunkRegistrationMode::StaticPeer,
            peer: None,
            registered: false,
            last_sip_status: None,
            started_at: None,
            registered_at: None,
            expires_at: None,
            next_retry_at: None,
            last_error: None,
            register_attempts: 0,
            reconnect_count: 0,
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
}

impl From<&TrunkSnapshot> for TrunkRuntimeStatus {
    fn from(snapshot: &TrunkSnapshot) -> Self {
        Self {
            phase: snapshot.phase.as_str().to_string(),
            stage: snapshot.stage.as_str().to_string(),
            enabled: snapshot.enabled,
            registration_mode: mode_name(snapshot.registration_mode).to_string(),
            peer: snapshot.peer.clone(),
            registered: snapshot.registered,
            last_sip_status: snapshot.last_sip_status,
            started_at: snapshot.started_at.clone(),
            registered_at: snapshot.registered_at.clone(),
            expires_at: snapshot.expires_at.clone(),
            next_retry_at: snapshot.next_retry_at.clone(),
            last_error: snapshot.last_error.clone(),
            register_attempts: snapshot.register_attempts,
            reconnect_count: snapshot.reconnect_count,
        }
    }
}

#[derive(Clone, Default)]
pub struct TrunkRuntime {
    snapshot: Arc<RwLock<TrunkSnapshot>>,
    operation_lock: Arc<Mutex<()>>,
    generation: Arc<AtomicU64>,
}

impl TrunkRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self) -> TrunkSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn status(&self) -> TrunkRuntimeStatus {
        TrunkRuntimeStatus::from(&*self.snapshot.read().await)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub async fn operation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.operation_lock.lock().await
    }

    /// Apply persisted intent and cancel any future driver using the previous
    /// generation. Enabling stops at `configured` until the D4 driver starts.
    pub async fn apply_profile(&self, profile: &TrunkProfileConfig) -> TrunkSnapshot {
        self.generation.fetch_add(1, Ordering::SeqCst);
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

    /// Startup/hotplug reconciliation that does not disturb an already active
    /// D4 session. Explicit config changes continue to use `apply_profile`.
    pub async fn reconcile_profile(&self, profile: &TrunkProfileConfig) -> TrunkSnapshot {
        let snapshot = self.snapshot().await;
        if snapshot.enabled != profile.enabled {
            return self.apply_profile(profile).await;
        }
        snapshot
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
