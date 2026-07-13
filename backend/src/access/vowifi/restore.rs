use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorePhase {
    Idle,
    Snapshot,
    TeardownVowifi,
    ProfileEnable,
    CardResetSettling,
    IdentityRefresh,
    SimAuthGate,
    RuntimeRestore,
    VerifyRegister,
    SmsReady,
    Degraded,
    RetryScheduled,
    Failed,
}

impl RestorePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            RestorePhase::Idle => "idle",
            RestorePhase::Snapshot => "snapshot",
            RestorePhase::TeardownVowifi => "teardown_vowifi",
            RestorePhase::ProfileEnable => "profile_enable",
            RestorePhase::CardResetSettling => "card_reset_settling",
            RestorePhase::IdentityRefresh => "identity_refresh",
            RestorePhase::SimAuthGate => "sim_auth_gate",
            RestorePhase::RuntimeRestore => "runtime_restore",
            RestorePhase::VerifyRegister => "verify_register",
            RestorePhase::SmsReady => "sms_ready",
            RestorePhase::Degraded => "degraded",
            RestorePhase::RetryScheduled => "retry_scheduled",
            RestorePhase::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RestoreProgress {
    pub phase: RestorePhase,
    pub switch_token: Option<String>,
    pub phase_started_at: Option<Instant>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub degraded_reason: Option<String>,
    pub retry_count: u8,
}

impl Default for RestoreProgress {
    fn default() -> Self {
        Self {
            phase: RestorePhase::Idle,
            switch_token: None,
            phase_started_at: None,
            identity_ready: false,
            sim_auth_ready: false,
            degraded_reason: None,
            retry_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreProgressResponse {
    pub switch_phase: Option<&'static str>,
    pub switch_token: Option<String>,
    pub phase_ms: Option<u64>,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub degraded_reason: Option<String>,
    pub retry_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreRuntimeSnapshotSummary {
    pub previous_runtime_active: bool,
    pub previous_tunnel_present: bool,
    pub previous_ims_registered: bool,
    pub previous_sms_ready: bool,
    pub previous_sms_mode: &'static str,
    pub profile_generation_captured: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreCleanupSummary {
    pub runtime_teardown_done: bool,
    pub qmi_sms_restored: bool,
    pub apdu_sessions_cleared: bool,
    pub stale_runtime_reuse_allowed: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreGateSummary {
    pub identity_ready: bool,
    pub identity_changed: bool,
    pub sim_auth_ready: bool,
    pub home_plmn_source: &'static str,
    pub card_reset_settling_ms: u64,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreRuntimeAttemptSummary {
    pub attempts: u8,
    pub first_failure_retryable: bool,
    pub first_failure_reason: Option<&'static str>,
    pub final_register_verified: bool,
    pub final_sms_ready: bool,
    pub rebuild_strategy: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreWorkflowEvent {
    pub phase: &'static str,
    pub phase_ms: u64,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub retry_count: u8,
    pub sms_mode: &'static str,
    pub cleanup_done: bool,
    pub register_verified: bool,
    pub sms_ready: bool,
    pub degraded_reason: Option<&'static str>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EsimRestorePublicState {
    pub switch_token: String,
    pub switch_phase: &'static str,
    pub phase_ms: u64,
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub degraded_reason: Option<&'static str>,
    pub retry_count: u8,
    pub snapshot: RestoreRuntimeSnapshotSummary,
    pub cleanup: RestoreCleanupSummary,
    pub gate: RestoreGateSummary,
    pub runtime_restore: RestoreRuntimeAttemptSummary,
    pub events: Vec<RestoreWorkflowEvent>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreWorkflowError {
    InvalidPhase {
        expected: &'static str,
        actual: &'static str,
    },
    MissingSnapshot,
    IdentityNotReady,
    SimAuthNotReady,
}

impl std::fmt::Display for RestoreWorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPhase { expected, actual } => {
                write!(
                    f,
                    "invalid restore phase expected={expected} actual={actual}"
                )
            }
            Self::MissingSnapshot => write!(f, "runtime snapshot is required before teardown"),
            Self::IdentityNotReady => write!(f, "identity gate is not ready"),
            Self::SimAuthNotReady => write!(f, "SIMAuth gate is not ready"),
        }
    }
}

impl std::error::Error for RestoreWorkflowError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsimRestoreStateMachine {
    switch_token: String,
    phase: RestorePhase,
    phase_ms: u64,
    identity_ready: bool,
    identity_changed: bool,
    sim_auth_ready: bool,
    degraded_reason: Option<&'static str>,
    retry_count: u8,
    sms_mode: &'static str,
    snapshot: Option<RestoreRuntimeSnapshotSummary>,
    cleanup: RestoreCleanupSummary,
    gate: RestoreGateSummary,
    runtime_restore: RestoreRuntimeAttemptSummary,
    events: Vec<RestoreWorkflowEvent>,
}

impl EsimRestoreStateMachine {
    pub fn new(switch_token: impl Into<String>) -> Self {
        Self {
            switch_token: switch_token.into(),
            phase: RestorePhase::Idle,
            phase_ms: 0,
            identity_ready: false,
            identity_changed: false,
            sim_auth_ready: false,
            degraded_reason: None,
            retry_count: 0,
            sms_mode: "vowifi",
            snapshot: None,
            cleanup: RestoreCleanupSummary {
                runtime_teardown_done: false,
                qmi_sms_restored: false,
                apdu_sessions_cleared: false,
                stale_runtime_reuse_allowed: false,
                sensitive_values_policy: "cleanup_flags_only_no_apdu_or_identity_values",
            },
            gate: RestoreGateSummary {
                identity_ready: false,
                identity_changed: false,
                sim_auth_ready: false,
                home_plmn_source: "not_ready",
                card_reset_settling_ms: 0,
                sensitive_values_policy: "gate_metadata_only_no_imsi_or_iccid",
            },
            runtime_restore: RestoreRuntimeAttemptSummary {
                attempts: 0,
                first_failure_retryable: false,
                first_failure_reason: None,
                final_register_verified: false,
                final_sms_ready: false,
                rebuild_strategy: "full_teardown_then_rebuild",
                sensitive_values_policy: "attempt_results_only_no_runtime_secrets",
            },
            events: Vec::new(),
        }
    }

    pub fn snapshot_old_runtime(&mut self) {
        self.phase = RestorePhase::Snapshot;
        self.phase_ms = 8;
        self.snapshot = Some(RestoreRuntimeSnapshotSummary {
            previous_runtime_active: true,
            previous_tunnel_present: true,
            previous_ims_registered: true,
            previous_sms_ready: true,
            previous_sms_mode: "vowifi",
            profile_generation_captured: true,
            sensitive_values_policy: "only_runtime_booleans_no_sim_identifiers",
        });
        self.push_event();
    }

    pub fn teardown_old_runtime(&mut self) -> Result<(), RestoreWorkflowError> {
        if self.snapshot.is_none() {
            return Err(RestoreWorkflowError::MissingSnapshot);
        }
        self.phase = RestorePhase::TeardownVowifi;
        self.phase_ms = 42;
        self.sms_mode = "qmi_at_fallback";
        self.cleanup.runtime_teardown_done = true;
        self.cleanup.qmi_sms_restored = true;
        self.push_event();
        Ok(())
    }

    pub fn clear_apdu_sessions(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::TeardownVowifi)?;
        self.cleanup.apdu_sessions_cleared = true;
        self.phase_ms = 51;
        self.push_event();
        Ok(())
    }

    pub fn enable_profile(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_cleanup()?;
        self.phase = RestorePhase::ProfileEnable;
        self.phase_ms = 160;
        self.push_event();
        Ok(())
    }

    pub fn settle_card_reset(&mut self, settling_ms: u64) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::ProfileEnable)?;
        self.phase = RestorePhase::CardResetSettling;
        self.phase_ms = settling_ms;
        self.gate.card_reset_settling_ms = settling_ms;
        self.push_event();
        Ok(())
    }

    pub fn refresh_identity(&mut self, changed: bool) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::CardResetSettling)?;
        self.phase = RestorePhase::IdentityRefresh;
        self.phase_ms = 220;
        self.identity_ready = true;
        self.identity_changed = changed;
        self.gate.identity_ready = true;
        self.gate.identity_changed = changed;
        self.gate.home_plmn_source = "imsi_derived_home_plmn";
        self.push_event();
        Ok(())
    }

    pub fn pass_sim_auth_gate(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::IdentityRefresh)?;
        if !self.identity_ready {
            return Err(RestoreWorkflowError::IdentityNotReady);
        }
        self.phase = RestorePhase::SimAuthGate;
        self.phase_ms = 275;
        self.sim_auth_ready = true;
        self.gate.sim_auth_ready = true;
        self.push_event();
        Ok(())
    }

    pub fn start_runtime_restore(&mut self) -> Result<(), RestoreWorkflowError> {
        if !self.identity_ready {
            return Err(RestoreWorkflowError::IdentityNotReady);
        }
        if !self.sim_auth_ready {
            return Err(RestoreWorkflowError::SimAuthNotReady);
        }
        self.phase = RestorePhase::RuntimeRestore;
        self.phase_ms = 340;
        self.runtime_restore.attempts = self.runtime_restore.attempts.saturating_add(1);
        self.degraded_reason = None;
        self.push_event();
        Ok(())
    }

    pub fn mark_context_canceled(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::RuntimeRestore)?;
        self.phase = RestorePhase::Degraded;
        self.phase_ms = 390;
        self.degraded_reason = Some("context_canceled");
        self.runtime_restore.first_failure_retryable = true;
        self.runtime_restore.first_failure_reason = Some("context_canceled");
        self.push_event();
        Ok(())
    }

    pub fn schedule_retry(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::Degraded)?;
        self.phase = RestorePhase::RetryScheduled;
        self.phase_ms = 430;
        self.retry_count = self.retry_count.saturating_add(1);
        self.push_event();
        Ok(())
    }

    pub fn retry_teardown_before_restore(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::RetryScheduled)?;
        self.phase = RestorePhase::TeardownVowifi;
        self.phase_ms = 455;
        self.sms_mode = "qmi_at_fallback";
        self.cleanup.runtime_teardown_done = true;
        self.cleanup.qmi_sms_restored = true;
        self.cleanup.apdu_sessions_cleared = true;
        self.degraded_reason = None;
        self.push_event();
        Ok(())
    }

    pub fn verify_register(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::RuntimeRestore)?;
        self.phase = RestorePhase::VerifyRegister;
        self.phase_ms = 620;
        self.runtime_restore.final_register_verified = true;
        self.push_event();
        Ok(())
    }

    pub fn mark_sms_ready(&mut self) -> Result<(), RestoreWorkflowError> {
        self.require_phase(RestorePhase::VerifyRegister)?;
        self.phase = RestorePhase::SmsReady;
        self.phase_ms = 700;
        self.sms_mode = "vowifi";
        self.runtime_restore.final_sms_ready = true;
        self.push_event();
        Ok(())
    }

    pub fn snapshot(&self) -> EsimRestorePublicState {
        EsimRestorePublicState {
            switch_token: self.switch_token.clone(),
            switch_phase: self.phase.as_str(),
            phase_ms: self.phase_ms,
            identity_ready: self.identity_ready,
            sim_auth_ready: self.sim_auth_ready,
            degraded_reason: self.degraded_reason,
            retry_count: self.retry_count,
            snapshot: self
                .snapshot
                .clone()
                .unwrap_or(RestoreRuntimeSnapshotSummary {
                    previous_runtime_active: false,
                    previous_tunnel_present: false,
                    previous_ims_registered: false,
                    previous_sms_ready: false,
                    previous_sms_mode: "unknown",
                    profile_generation_captured: false,
                    sensitive_values_policy: "only_runtime_booleans_no_sim_identifiers",
                }),
            cleanup: self.cleanup.clone(),
            gate: self.gate.clone(),
            runtime_restore: self.runtime_restore.clone(),
            events: self.events.clone(),
            sensitive_values_policy: "dry_run_switch_token_no_eid_iccid_imsi_or_apdu_values",
        }
    }

    fn require_phase(&self, expected: RestorePhase) -> Result<(), RestoreWorkflowError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(RestoreWorkflowError::InvalidPhase {
                expected: expected.as_str(),
                actual: self.phase.as_str(),
            })
        }
    }

    fn require_cleanup(&self) -> Result<(), RestoreWorkflowError> {
        if self.cleanup.runtime_teardown_done
            && self.cleanup.qmi_sms_restored
            && self.cleanup.apdu_sessions_cleared
        {
            Ok(())
        } else {
            Err(RestoreWorkflowError::InvalidPhase {
                expected: "teardown_with_apdu_cleanup",
                actual: self.phase.as_str(),
            })
        }
    }

    fn push_event(&mut self) {
        self.events.push(RestoreWorkflowEvent {
            phase: self.phase.as_str(),
            phase_ms: self.phase_ms,
            identity_ready: self.identity_ready,
            sim_auth_ready: self.sim_auth_ready,
            retry_count: self.retry_count,
            sms_mode: self.sms_mode,
            cleanup_done: self.cleanup.runtime_teardown_done
                && self.cleanup.qmi_sms_restored
                && self.cleanup.apdu_sessions_cleared,
            register_verified: self.runtime_restore.final_register_verified,
            sms_ready: self.runtime_restore.final_sms_ready,
            degraded_reason: self.degraded_reason,
            sensitive_values_policy: "phase_metadata_only_no_device_or_profile_identifiers",
        });
    }
}

pub fn build_dry_run_restore_snapshot() -> EsimRestorePublicState {
    let mut machine = EsimRestoreStateMachine::new("dry_run_switch_token");
    machine.snapshot_old_runtime();
    machine
        .teardown_old_runtime()
        .expect("dry-run snapshot allows teardown");
    machine
        .clear_apdu_sessions()
        .expect("dry-run teardown allows APDU cleanup");
    machine
        .enable_profile()
        .expect("dry-run cleanup allows profile enable");
    machine
        .settle_card_reset(1_000)
        .expect("dry-run profile enable allows card settling");
    machine
        .refresh_identity(true)
        .expect("dry-run settling allows identity refresh");
    machine
        .pass_sim_auth_gate()
        .expect("dry-run identity allows SIMAuth gate");
    machine
        .start_runtime_restore()
        .expect("dry-run gates allow restore");
    machine
        .mark_context_canceled()
        .expect("dry-run first restore can be canceled");
    machine
        .schedule_retry()
        .expect("dry-run cancellation schedules retry");
    machine
        .retry_teardown_before_restore()
        .expect("dry-run retry performs teardown before rebuild");
    machine
        .start_runtime_restore()
        .expect("dry-run retry gates remain ready");
    machine
        .verify_register()
        .expect("dry-run retry verifies IMS REGISTER");
    machine
        .mark_sms_ready()
        .expect("dry-run retry reaches SMS ready");
    machine.snapshot()
}

impl RestoreProgress {
    pub fn enter_phase(&mut self, phase: RestorePhase, switch_token: Option<String>) {
        self.phase = phase;
        if switch_token.is_some() {
            self.switch_token = switch_token;
        }
        self.phase_started_at = Some(Instant::now());
        self.degraded_reason = None;
    }

    pub fn mark_degraded(&mut self, reason: impl Into<String>) {
        self.phase = RestorePhase::Degraded;
        self.degraded_reason = Some(reason.into());
        self.phase_started_at = Some(Instant::now());
    }

    pub fn schedule_retry(&mut self) {
        self.phase = RestorePhase::RetryScheduled;
        self.retry_count = self.retry_count.saturating_add(1);
        self.phase_started_at = Some(Instant::now());
    }

    pub fn response(&self) -> RestoreProgressResponse {
        RestoreProgressResponse {
            switch_phase: (self.phase != RestorePhase::Idle).then_some(self.phase.as_str()),
            switch_token: self.switch_token.clone(),
            phase_ms: self.phase_started_at.map(|started| {
                duration_millis_saturating(Instant::now().saturating_duration_since(started))
            }),
            identity_ready: self.identity_ready,
            sim_auth_ready: self.sim_auth_ready,
            degraded_reason: self.degraded_reason.clone(),
            retry_count: self.retry_count,
        }
    }
}

fn duration_millis_saturating(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_restore_has_no_switch_phase() {
        let progress = RestoreProgress::default();
        let response = progress.response();

        assert_eq!(response.switch_phase, None);
        assert_eq!(response.switch_token, None);
    }

    #[test]
    fn context_cancellation_can_degrade_and_retry() {
        let mut progress = RestoreProgress::default();
        progress.enter_phase(RestorePhase::RuntimeRestore, Some("switch-1".to_string()));
        progress.mark_degraded("context_canceled");

        let degraded = progress.response();
        assert_eq!(degraded.switch_phase, Some("degraded"));
        assert_eq!(
            degraded.degraded_reason.as_deref(),
            Some("context_canceled")
        );

        progress.schedule_retry();
        let retry = progress.response();
        assert_eq!(retry.switch_phase, Some("retry_scheduled"));
        assert_eq!(retry.retry_count, 1);
        assert_eq!(retry.switch_token.as_deref(), Some("switch-1"));
    }

    #[test]
    fn dry_run_restore_retries_context_cancellation_and_reaches_sms_ready() {
        let snapshot = build_dry_run_restore_snapshot();

        assert_eq!(snapshot.switch_phase, "sms_ready");
        assert_eq!(snapshot.retry_count, 1);
        assert!(snapshot.identity_ready);
        assert!(snapshot.sim_auth_ready);
        assert!(snapshot.cleanup.runtime_teardown_done);
        assert!(snapshot.cleanup.qmi_sms_restored);
        assert!(snapshot.cleanup.apdu_sessions_cleared);
        assert!(!snapshot.cleanup.stale_runtime_reuse_allowed);
        assert_eq!(snapshot.gate.card_reset_settling_ms, 1_000);
        assert_eq!(snapshot.gate.home_plmn_source, "imsi_derived_home_plmn");
        assert_eq!(snapshot.runtime_restore.attempts, 2);
        assert_eq!(
            snapshot.runtime_restore.first_failure_reason,
            Some("context_canceled")
        );
        assert!(snapshot.runtime_restore.first_failure_retryable);
        assert!(snapshot.runtime_restore.final_register_verified);
        assert!(snapshot.runtime_restore.final_sms_ready);
        assert!(snapshot
            .events
            .iter()
            .any(|event| event.phase == "degraded"
                && event.degraded_reason == Some("context_canceled")));
        assert!(snapshot
            .events
            .iter()
            .any(|event| event.phase == "retry_scheduled" && event.retry_count == 1));
    }

    #[test]
    fn restore_workflow_requires_identity_and_sim_auth_gates() {
        let mut machine = EsimRestoreStateMachine::new("dry_run_switch_token");

        assert!(matches!(
            machine.start_runtime_restore().unwrap_err(),
            RestoreWorkflowError::IdentityNotReady
        ));

        machine.snapshot_old_runtime();
        machine.teardown_old_runtime().expect("teardown");
        machine.clear_apdu_sessions().expect("cleanup");
        machine.enable_profile().expect("enable");
        machine.settle_card_reset(1_000).expect("settle");
        machine.refresh_identity(true).expect("identity");

        assert!(matches!(
            machine.start_runtime_restore().unwrap_err(),
            RestoreWorkflowError::SimAuthNotReady
        ));
    }

    #[test]
    fn dry_run_restore_snapshot_serializes_no_sensitive_identifiers() {
        let snapshot = build_dry_run_restore_snapshot();
        let json = serde_json::to_string(&snapshot).expect("serialize restore snapshot");
        let lower = json.to_ascii_lowercase();

        for forbidden in [
            "\"imsi\"",
            "\"iccid\"",
            "\"eid\"",
            "\"imei\"",
            "\"msisdn\"",
            "\"apdu\"",
            "\"spi\"",
            "\"ck\"",
            "\"ik\"",
            "\"password\"",
            "\"authorization\"",
        ] {
            assert!(
                !lower.contains(forbidden),
                "restore snapshot must not expose {forbidden}"
            );
        }

        assert!(!lower.contains("context canceled"));
        assert!(lower.contains("context_canceled"));
    }
}
