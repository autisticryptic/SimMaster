#![allow(dead_code)]

use std::{env, future::Future, pin::Pin};

use serde::Serialize;

use super::{
    dataplane::{ChildSaPublicState, ChildSaStateMachine},
    ike_state::IkePublicSnapshot,
    ims::ImsRegisterPublicState,
    live::{
        gate_blocker_for_stage, live_device_change_implementation_available,
        live_network_implementation_available, live_runtime_implementation_complete,
        LiveNetworkStageAdapter, LiveStageRunner, StatusProbeDatagramAdapter,
        SystemLiveDatagramAdapter, SystemLiveEpdgAdapter,
    },
    profiles::{self, CarrierProfile, GB_EE_23433},
    restore::EsimRestorePublicState,
    sms::SmsRuntimePublicState,
    voice::VoiceRuntimePublicState,
};

pub type ExecutorFuture<'a> = Pin<Box<dyn Future<Output = ExecutorStageResult> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorMode {
    DisabledNoop,
    DryRun,
    LivePlanned,
    Live,
}

impl ExecutorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DisabledNoop => "disabled_noop",
            Self::DryRun => "dry_run",
            Self::LivePlanned => "live_planned",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorStage {
    EsimRestore,
    SimAuth,
    Epdg,
    Ike,
    ChildSa,
    Esp,
    ImsRegister,
    Sms,
    Voice,
}

impl ExecutorStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EsimRestore => "esim_restore",
            Self::SimAuth => "sim_auth",
            Self::Epdg => "epdg",
            Self::Ike => "ike",
            Self::ChildSa => "child_sa",
            Self::Esp => "esp",
            Self::ImsRegister => "ims_register",
            Self::Sms => "sms",
            Self::Voice => "voice",
        }
    }

    pub fn component(self) -> &'static str {
        match self {
            Self::EsimRestore => "esim_restore",
            Self::SimAuth => "usim_aka",
            Self::Epdg => "epdg_transport",
            Self::Ike => "ikev2_eap_aka",
            Self::ChildSa => "child_sa",
            Self::Esp => "userspace_esp",
            Self::ImsRegister => "ims_register",
            Self::Sms => "sms_over_ims",
            Self::Voice => "voice_over_ims",
        }
    }
}

pub const EXECUTOR_STAGES: &[ExecutorStage] = &[
    ExecutorStage::EsimRestore,
    ExecutorStage::SimAuth,
    ExecutorStage::Epdg,
    ExecutorStage::Ike,
    ExecutorStage::ChildSa,
    ExecutorStage::Esp,
    ExecutorStage::ImsRegister,
    ExecutorStage::Sms,
    ExecutorStage::Voice,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutorCapability {
    pub stage: &'static str,
    pub component: &'static str,
    pub enabled: bool,
    pub mode: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveExecutorGateReport {
    pub live_network_authorized: bool,
    pub device_state_changes_authorized: bool,
    pub adb_path_configured: bool,
    pub device_admin_url_configured: bool,
    pub implementation_ready: bool,
    pub effective_live_network_allowed: bool,
    pub effective_device_state_changes_allowed: bool,
    pub blockers: Vec<&'static str>,
    pub sensitive_values_policy: &'static str,
}

impl Default for LiveExecutorGateReport {
    fn default() -> Self {
        Self::disabled()
    }
}

impl LiveExecutorGateReport {
    pub fn disabled() -> Self {
        Self {
            live_network_authorized: false,
            device_state_changes_authorized: false,
            adb_path_configured: false,
            device_admin_url_configured: false,
            implementation_ready: false,
            effective_live_network_allowed: false,
            effective_device_state_changes_allowed: false,
            blockers: vec![
                "live_network_authorization_missing",
                "live_runtime_executor_not_implemented",
            ],
            sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
        }
    }

    pub fn from_environment() -> Self {
        let live_network_authorized = env_flag_default_true("SIMADMIN_VOWIFI_LIVE_NETWORK_ALLOWED");
        let device_state_changes_authorized =
            env_flag_default_true("SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED");
        let adb_path_configured = env_non_empty("SIMADMIN_VOWIFI_ADB_PATH");
        let device_admin_url_configured = env_non_empty("SIMADMIN_VOWIFI_DEVICE_ADMIN_URL");
        let implementation_ready = live_runtime_implementation_complete();
        let live_network_implementation_ready = live_network_implementation_available();
        let device_change_implementation_ready = live_device_change_implementation_available();
        let effective_live_network_allowed =
            live_network_authorized && live_network_implementation_ready;
        let effective_device_state_changes_allowed =
            device_state_changes_authorized && device_change_implementation_ready;
        let mut blockers = Vec::new();
        if !live_network_authorized {
            blockers.push("live_network_authorization_missing");
        }
        if !device_state_changes_authorized {
            blockers.push("device_state_change_authorization_missing");
        }
        if !live_network_implementation_ready {
            blockers.push("live_network_executor_not_implemented");
        }
        if !device_change_implementation_ready {
            blockers.push("device_state_change_executor_not_implemented");
        }
        if !implementation_ready {
            blockers.push("live_runtime_partially_implemented");
        }

        Self {
            live_network_authorized,
            device_state_changes_authorized,
            adb_path_configured,
            device_admin_url_configured,
            implementation_ready,
            effective_live_network_allowed,
            effective_device_state_changes_allowed,
            blockers,
            sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeExecutorReport {
    pub executor_id: &'static str,
    pub mode: &'static str,
    pub live_network_allowed: bool,
    pub device_state_changes_allowed: bool,
    pub live_gate: LiveExecutorGateReport,
    pub capabilities: Vec<ExecutorCapability>,
    pub ike_dry_run: Option<IkePublicSnapshot>,
    pub dataplane_dry_run: Option<ChildSaPublicState>,
    pub ims_register_dry_run: Option<ImsRegisterPublicState>,
    pub sms_dry_run: Option<SmsRuntimePublicState>,
    pub voice_dry_run: Option<VoiceRuntimePublicState>,
    pub esim_restore_dry_run: Option<EsimRestorePublicState>,
}

impl Default for RuntimeExecutorReport {
    fn default() -> Self {
        NoopRuntimeExecutor.describe()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorStageRequest {
    pub stage: ExecutorStage,
    pub profile_id: Option<String>,
    pub plmn: Option<String>,
    pub trace_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorStageStatus {
    Skipped,
    Completed,
    Failed,
}

impl ExecutorStageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutorSoakObservation {
    pub scenario_id: &'static str,
    pub sample_kind: &'static str,
    pub metric_name: &'static str,
    pub metric_value: i64,
    pub state: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutorStageResult {
    pub stage: &'static str,
    pub status: &'static str,
    pub readiness_key: &'static str,
    pub reason: Option<String>,
    pub soak_observation: Option<ExecutorSoakObservation>,
}

pub trait RuntimeExecutor: Send + Sync {
    fn describe(&self) -> RuntimeExecutorReport;

    fn run_stage<'a>(&'a self, request: ExecutorStageRequest) -> ExecutorFuture<'a>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopRuntimeExecutor;

impl RuntimeExecutor for NoopRuntimeExecutor {
    fn describe(&self) -> RuntimeExecutorReport {
        RuntimeExecutorReport {
            executor_id: "noop_runtime_executor",
            mode: ExecutorMode::DisabledNoop.as_str(),
            live_network_allowed: false,
            device_state_changes_allowed: false,
            live_gate: LiveExecutorGateReport::disabled(),
            ike_dry_run: None,
            dataplane_dry_run: None,
            ims_register_dry_run: None,
            sms_dry_run: None,
            voice_dry_run: None,
            esim_restore_dry_run: None,
            capabilities: EXECUTOR_STAGES
                .iter()
                .copied()
                .map(|stage| ExecutorCapability {
                    stage: stage.as_str(),
                    component: stage.component(),
                    enabled: false,
                    mode: ExecutorMode::DisabledNoop.as_str(),
                    reason: "executor_disabled_until_live_runtime_is_enabled",
                })
                .collect(),
        }
    }

    fn run_stage<'a>(&'a self, request: ExecutorStageRequest) -> ExecutorFuture<'a> {
        Box::pin(async move {
            ExecutorStageResult {
                stage: request.stage.as_str(),
                status: ExecutorStageStatus::Skipped.as_str(),
                readiness_key: readiness_key_for_stage(request.stage),
                reason: Some("noop_executor_does_not_touch_device_or_network".to_string()),
                soak_observation: None,
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DryRunRuntimeExecutor {
    profile: &'static CarrierProfile,
}

impl Default for DryRunRuntimeExecutor {
    fn default() -> Self {
        Self {
            profile: &GB_EE_23433,
        }
    }
}

impl DryRunRuntimeExecutor {
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self { profile }
    }

    pub fn build_ike_snapshot(&self) -> IkePublicSnapshot {
        let mut machine = super::ike_state::IkeStateMachine::new(
            self.profile,
            0x1111_2222_3333_4444,
            vec![0x55; 32],
            vec![0x66; 256],
        );
        let request = machine
            .build_sa_init_request()
            .expect("dry-run IKE SA_INIT request uses static profile data");
        let response = dry_run_sa_init_response(&request);
        machine
            .accept_sa_init_response(&response)
            .expect("dry-run IKE SA_INIT response is internally generated");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("dry-run shared secret is synthetic");
        machine.snapshot()
    }

    pub fn build_dataplane_snapshot(&self) -> ChildSaPublicState {
        let mut state = ChildSaStateMachine::new(self.profile);
        let mut sample_inner_packet = vec![0u8; 240];
        sample_inner_packet[0] = 0x60;
        state
            .negotiate_child_sa(0x1111_2222, 0x3333_4444)
            .expect("dry-run CHILD_SA identifiers are synthetic and nonzero");
        state
            .mark_esp_secrets_ready()
            .expect("dry-run ESP metadata follows CHILD_SA negotiation");
        state
            .mark_inner_stack_ready()
            .expect("dry-run smoltcp gateway follows ESP readiness");
        state
            .record_inbound_esp_frame(1, 152)
            .expect("dry-run inbound ESP metadata starts at sequence one");
        state
            .record_outbound_inner_packet(&sample_inner_packet)
            .expect("dry-run outbound inner packet stays within MTU");
        state.snapshot()
    }

    pub fn build_ims_register_snapshot(&self) -> ImsRegisterPublicState {
        super::ims::build_dry_run_register_snapshot(self.profile)
    }

    pub fn build_sms_snapshot(&self) -> SmsRuntimePublicState {
        super::sms::build_dry_run_sms_snapshot(self.profile)
    }

    pub fn build_voice_snapshot(&self) -> VoiceRuntimePublicState {
        super::voice::build_dry_run_voice_snapshot(self.profile)
    }

    pub fn build_esim_restore_snapshot(&self) -> EsimRestorePublicState {
        super::restore::build_dry_run_restore_snapshot()
    }
}

impl RuntimeExecutor for DryRunRuntimeExecutor {
    fn describe(&self) -> RuntimeExecutorReport {
        RuntimeExecutorReport {
            executor_id: "dry_run_runtime_executor",
            mode: ExecutorMode::DryRun.as_str(),
            live_network_allowed: false,
            device_state_changes_allowed: false,
            live_gate: LiveExecutorGateReport::disabled(),
            capabilities: EXECUTOR_STAGES
                .iter()
                .copied()
                .map(|stage| ExecutorCapability {
                    stage: stage.as_str(),
                    component: stage.component(),
                    enabled: dry_run_stage_enabled(stage),
                    mode: ExecutorMode::DryRun.as_str(),
                    reason: dry_run_stage_reason(stage),
                })
                .collect(),
            ike_dry_run: Some(self.build_ike_snapshot()),
            dataplane_dry_run: Some(self.build_dataplane_snapshot()),
            ims_register_dry_run: Some(self.build_ims_register_snapshot()),
            sms_dry_run: Some(self.build_sms_snapshot()),
            voice_dry_run: Some(self.build_voice_snapshot()),
            esim_restore_dry_run: Some(self.build_esim_restore_snapshot()),
        }
    }

    fn run_stage<'a>(&'a self, request: ExecutorStageRequest) -> ExecutorFuture<'a> {
        Box::pin(async move {
            let enabled = dry_run_stage_enabled(request.stage);
            ExecutorStageResult {
                stage: request.stage.as_str(),
                status: if enabled {
                    ExecutorStageStatus::Completed.as_str()
                } else {
                    ExecutorStageStatus::Skipped.as_str()
                },
                readiness_key: readiness_key_for_stage(request.stage),
                reason: Some(dry_run_stage_run_reason(request.stage).to_string()),
                soak_observation: Some(soak_observation_for_stage(request.stage)),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct LiveRuntimeExecutor {
    gate: LiveExecutorGateReport,
}

impl LiveRuntimeExecutor {
    pub fn from_gate(gate: LiveExecutorGateReport) -> Self {
        Self { gate }
    }

    fn stage_enabled(&self, stage: ExecutorStage) -> bool {
        gate_blocker_for_stage(stage, &self.gate).is_none()
    }
}

impl RuntimeExecutor for LiveRuntimeExecutor {
    fn describe(&self) -> RuntimeExecutorReport {
        let mode = if self.gate.implementation_ready {
            ExecutorMode::Live.as_str()
        } else {
            ExecutorMode::LivePlanned.as_str()
        };
        RuntimeExecutorReport {
            executor_id: "live_runtime_executor",
            mode,
            live_network_allowed: self.gate.effective_live_network_allowed,
            device_state_changes_allowed: self.gate.effective_device_state_changes_allowed,
            live_gate: self.gate.clone(),
            capabilities: EXECUTOR_STAGES
                .iter()
                .copied()
                .map(|stage| ExecutorCapability {
                    stage: stage.as_str(),
                    component: stage.component(),
                    enabled: self.stage_enabled(stage),
                    mode,
                    reason: live_stage_reason(stage, &self.gate),
                })
                .collect(),
            ike_dry_run: None,
            dataplane_dry_run: None,
            ims_register_dry_run: None,
            sms_dry_run: None,
            voice_dry_run: None,
            esim_restore_dry_run: None,
        }
    }

    fn run_stage<'a>(&'a self, request: ExecutorStageRequest) -> ExecutorFuture<'a> {
        Box::pin(async move {
            let profile = profile_for_stage_request(&request);
            if request.trace_id == "runtime-status-probe" && request.stage == ExecutorStage::Ike {
                let adapter =
                    LiveNetworkStageAdapter::new(SystemLiveEpdgAdapter, StatusProbeDatagramAdapter);
                return LiveStageRunner::new(self.gate.clone(), profile, adapter)
                    .run(request)
                    .await;
            }

            let adapter =
                LiveNetworkStageAdapter::new(SystemLiveEpdgAdapter, SystemLiveDatagramAdapter);
            LiveStageRunner::new(self.gate.clone(), profile, adapter)
                .run(request)
                .await
        })
    }
}

fn dry_run_stage_enabled(stage: ExecutorStage) -> bool {
    matches!(
        stage,
        ExecutorStage::EsimRestore
            | ExecutorStage::Ike
            | ExecutorStage::ChildSa
            | ExecutorStage::Esp
            | ExecutorStage::ImsRegister
            | ExecutorStage::Sms
            | ExecutorStage::Voice
    )
}

fn dry_run_stage_reason(stage: ExecutorStage) -> &'static str {
    match stage {
        ExecutorStage::EsimRestore => "offline_esim_restore_workflow_dry_run",
        ExecutorStage::Ike => "offline_ike_state_machine_dry_run",
        ExecutorStage::ChildSa => "offline_child_sa_state_machine_dry_run",
        ExecutorStage::Esp => "offline_userspace_esp_metadata_dry_run",
        ExecutorStage::ImsRegister => "offline_ims_register_sec_agree_dry_run",
        ExecutorStage::Sms => "offline_sms_over_ims_delivery_dry_run",
        ExecutorStage::Voice => "offline_voice_over_ims_call_dry_run",
        _ => "stage_not_part_of_controlplane_dataplane_dry_run",
    }
}

fn dry_run_stage_run_reason(stage: ExecutorStage) -> &'static str {
    match stage {
        ExecutorStage::EsimRestore => "offline_esim_restore_workflow_dry_run_completed",
        ExecutorStage::Ike => "offline_ike_state_machine_dry_run_completed",
        ExecutorStage::ChildSa => "offline_child_sa_state_machine_dry_run_completed",
        ExecutorStage::Esp => "offline_userspace_esp_metadata_dry_run_completed",
        ExecutorStage::ImsRegister => "offline_ims_register_sec_agree_dry_run_completed",
        ExecutorStage::Sms => "offline_sms_over_ims_delivery_dry_run_completed",
        ExecutorStage::Voice => "offline_voice_over_ims_call_dry_run_completed",
        _ => {
            "dry_run_executor_only_covers_esim_restore_ike_child_sa_esp_ims_register_sms_and_voice_stages"
        }
    }
}

fn live_stage_reason(stage: ExecutorStage, gate: &LiveExecutorGateReport) -> &'static str {
    gate_blocker_for_stage(stage, gate).unwrap_or("live_stage_available")
}

fn profile_for_stage_request(request: &ExecutorStageRequest) -> &'static CarrierProfile {
    request
        .profile_id
        .as_deref()
        .and_then(profiles::resolve_by_profile_id)
        .or_else(|| request.plmn.as_deref().and_then(profile_for_plmn))
        .unwrap_or(&GB_EE_23433)
}

fn profile_for_plmn(plmn: &str) -> Option<&'static CarrierProfile> {
    let digits = plmn
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.len() == 5 || digits.len() == 6 {
        profiles::resolve_by_plmn(&digits[..3], &digits[3..])
    } else {
        None
    }
}

pub fn soak_observation_for_stage(stage: ExecutorStage) -> ExecutorSoakObservation {
    let (scenario_id, metric_name) = match stage {
        ExecutorStage::EsimRestore => ("esim_restore_race_soak", "restore_stage_attempts"),
        ExecutorStage::SimAuth => ("esim_restore_race_soak", "sim_auth_gate_checks"),
        ExecutorStage::Epdg => ("network_path_recovery_soak", "epdg_resolution_attempts"),
        ExecutorStage::Ike => ("rekey_dpd_nat_t_soak", "ike_stage_attempts"),
        ExecutorStage::ChildSa => ("rekey_dpd_nat_t_soak", "child_sa_stage_attempts"),
        ExecutorStage::Esp => ("rekey_dpd_nat_t_soak", "esp_stage_attempts"),
        ExecutorStage::ImsRegister => ("register_refresh_soak", "register_stage_attempts"),
        ExecutorStage::Sms => ("sms_delivery_consistency_soak", "sms_stage_attempts"),
        ExecutorStage::Voice => ("voice_call_setup_soak", "voice_stage_attempts"),
    };

    ExecutorSoakObservation {
        scenario_id,
        sample_kind: "stage_result",
        metric_name,
        metric_value: 1,
        state: "planned_no_live_io",
        sensitive_values_policy: "counter_metadata_only_no_payload_or_secret_values",
    }
}

fn dry_run_sa_init_response(request: &super::ike_codec::IkeMessage) -> Vec<u8> {
    let sa_body = request
        .payloads
        .iter()
        .find(|payload| {
            payload.payload_type == super::ike_codec::IkePayloadType::SecurityAssociation
        })
        .expect("request has SA")
        .body
        .clone();
    super::ike_codec::IkeMessage {
        header: super::ike_codec::IkeHeader {
            initiator_spi: request.header.initiator_spi,
            responder_spi: 0x9999_aaaa_bbbb_cccc,
            next_payload: super::ike_codec::IkePayloadType::SecurityAssociation,
            major_version: 2,
            minor_version: 0,
            exchange_type: super::ike_codec::IkeExchangeType::IkeSaInit,
            flags: super::ike_codec::IkeFlags {
                initiator: false,
                response: true,
                version: false,
            },
            message_id: 0,
            length: 0,
        },
        payloads: vec![
            super::ike_codec::IkePayload {
                payload_type: super::ike_codec::IkePayloadType::SecurityAssociation,
                critical: false,
                body: sa_body,
            },
            super::ike_payloads::build_ke_payload(super::ike_payloads::DH_MODP_2048, &[0x88; 256]),
            super::ike_payloads::build_nonce_payload(&[0x99; 32]),
        ],
    }
    .encode()
    .expect("encode dry-run SA_INIT response")
}

fn env_flag(key: &str) -> bool {
    env::var(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_flag_default_true(key: &str) -> bool {
    env::var(key)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true)
}

fn env_non_empty(key: &str) -> bool {
    env::var(key)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

pub fn readiness_key_for_stage(stage: ExecutorStage) -> &'static str {
    match stage {
        ExecutorStage::EsimRestore => "esim_restore_ready",
        ExecutorStage::SimAuth => "sim_auth_ready",
        ExecutorStage::Epdg => "epdg_ready",
        ExecutorStage::Ike => "ike_ready",
        ExecutorStage::ChildSa => "child_sa_ready",
        ExecutorStage::Esp => "esp_ready",
        ExecutorStage::ImsRegister => "ims_registered",
        ExecutorStage::Sms => "sms_ready",
        ExecutorStage::Voice => "voice_ready",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_report_disables_all_runtime_stages() {
        let report = NoopRuntimeExecutor.describe();

        assert_eq!(report.executor_id, "noop_runtime_executor");
        assert_eq!(report.mode, "disabled_noop");
        assert!(!report.live_network_allowed);
        assert!(!report.device_state_changes_allowed);
        assert_eq!(report.capabilities.len(), EXECUTOR_STAGES.len());
        assert!(report
            .capabilities
            .iter()
            .all(|capability| !capability.enabled));
    }

    #[test]
    fn live_gate_from_environment_enables_only_implemented_network_stages() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().expect("env lock");
        let keys = [
            "SIMADMIN_VOWIFI_LIVE_NETWORK_ALLOWED",
            "SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED",
            "SIMADMIN_VOWIFI_ADB_PATH",
            "SIMADMIN_VOWIFI_DEVICE_ADMIN_URL",
        ];
        let previous = keys.map(|key| (key, env::var(key).ok()));

        env::set_var("SIMADMIN_VOWIFI_LIVE_NETWORK_ALLOWED", "true");
        env::set_var("SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED", "true");
        env::set_var("SIMADMIN_VOWIFI_ADB_PATH", "configured");
        env::set_var("SIMADMIN_VOWIFI_DEVICE_ADMIN_URL", "configured");

        let gate = LiveExecutorGateReport::from_environment();

        for (key, value) in previous {
            if let Some(value) = value {
                env::set_var(key, value);
            } else {
                env::remove_var(key);
            }
        }

        assert!(gate.live_network_authorized);
        assert!(gate.device_state_changes_authorized);
        assert!(gate.adb_path_configured);
        assert!(gate.device_admin_url_configured);
        assert!(gate.implementation_ready);
        assert!(gate.effective_live_network_allowed);
        assert!(gate.effective_device_state_changes_allowed);
        assert!(gate.blockers.is_empty());
    }

    #[tokio::test]
    async fn noop_stage_run_is_skipped_without_sensitive_context() {
        let result = NoopRuntimeExecutor
            .run_stage(ExecutorStageRequest {
                stage: ExecutorStage::Ike,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "trace-test".to_string(),
            })
            .await;

        assert_eq!(result.stage, "ike");
        assert_eq!(result.status, "skipped");
        assert_eq!(result.readiness_key, "ike_ready");
        assert!(result.soak_observation.is_none());
        assert_eq!(
            result.reason.as_deref(),
            Some("noop_executor_does_not_touch_device_or_network")
        );
    }

    #[test]
    fn dry_run_report_contains_offline_snapshots_without_live_permissions() {
        let report = DryRunRuntimeExecutor::default().describe();

        assert_eq!(report.executor_id, "dry_run_runtime_executor");
        assert_eq!(report.mode, "dry_run");
        assert!(!report.live_network_allowed);
        assert!(!report.device_state_changes_allowed);
        assert_eq!(
            report.ike_dry_run.as_ref().map(|snapshot| snapshot.phase),
            Some("session_keys_ready")
        );
        assert_eq!(
            report
                .dataplane_dry_run
                .as_ref()
                .map(|snapshot| snapshot.phase),
            Some("forwarding")
        );
        assert_eq!(
            report
                .ims_register_dry_run
                .as_ref()
                .map(|snapshot| snapshot.phase),
            Some("registered")
        );
        assert_eq!(
            report
                .ims_register_dry_run
                .as_ref()
                .and_then(|snapshot| snapshot.last_sip_status),
            Some(200)
        );
        assert_eq!(
            report
                .sms_dry_run
                .as_ref()
                .map(|snapshot| snapshot.sms_ready),
            Some(true)
        );
        assert_eq!(
            report
                .sms_dry_run
                .as_ref()
                .map(|snapshot| snapshot.pending_delivery_count),
            Some(0)
        );
        assert_eq!(
            report
                .esim_restore_dry_run
                .as_ref()
                .map(|snapshot| snapshot.switch_phase),
            Some("sms_ready")
        );
        assert_eq!(
            report
                .esim_restore_dry_run
                .as_ref()
                .map(|snapshot| snapshot.retry_count),
            Some(1)
        );
        for expected_stage in [
            "esim_restore",
            "ike",
            "child_sa",
            "esp",
            "ims_register",
            "sms",
        ] {
            assert!(report
                .capabilities
                .iter()
                .any(|capability| capability.stage == expected_stage && capability.enabled));
        }

        let json = serde_json::to_string(&report).expect("serialize dry-run report");
        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "skeyseed",
            "sk_d",
            "shared_secret",
            "key_material",
            "plaintext",
            "ciphertext",
            "authorization",
            "response",
            "initiator_nonce",
            "responder_nonce",
            "phone_number",
            "sender",
            "recipient",
            "content",
            "eid",
            "imei",
            "password",
            "token",
        ] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[tokio::test]
    async fn dry_run_completes_enabled_offline_stages() {
        let executor = DryRunRuntimeExecutor::default();

        for (stage, readiness_key) in [
            (ExecutorStage::EsimRestore, "esim_restore_ready"),
            (ExecutorStage::Ike, "ike_ready"),
            (ExecutorStage::ChildSa, "child_sa_ready"),
            (ExecutorStage::Esp, "esp_ready"),
            (ExecutorStage::ImsRegister, "ims_registered"),
            (ExecutorStage::Sms, "sms_ready"),
            (ExecutorStage::Voice, "voice_ready"),
        ] {
            let result = executor
                .run_stage(ExecutorStageRequest {
                    stage,
                    profile_id: Some("gb_ee_23433".to_string()),
                    plmn: Some("23433".to_string()),
                    trace_id: "dry-run".to_string(),
                })
                .await;
            assert_eq!(result.status, "completed");
            assert_eq!(result.readiness_key, readiness_key);
            assert!(result.soak_observation.is_some());
        }

        let sim_auth = executor
            .run_stage(ExecutorStageRequest {
                stage: ExecutorStage::SimAuth,
                profile_id: None,
                plmn: None,
                trace_id: "dry-run".to_string(),
            })
            .await;
        assert_eq!(sim_auth.status, "skipped");
    }

    #[test]
    fn stage_request_prefers_clean_room_profile_id_over_plmn() {
        let profile = profile_for_stage_request(&ExecutorStageRequest {
            stage: ExecutorStage::Epdg,
            profile_id: Some("nl_vodafone_20404".to_string()),
            plmn: Some("23433".to_string()),
            trace_id: "profile-id-selection".to_string(),
        });

        assert_eq!(profile.meta.profile_id, "nl_vodafone_20404");
        assert_eq!(profile.meta.plmn, "20404");
    }

    #[test]
    fn stage_request_can_select_profile_from_formatted_plmn() {
        let profile = profile_for_stage_request(&ExecutorStageRequest {
            stage: ExecutorStage::Epdg,
            profile_id: None,
            plmn: Some("204-04".to_string()),
            trace_id: "plmn-selection".to_string(),
        });

        assert_eq!(profile.meta.profile_id, "nl_vodafone_20404");
    }

    #[tokio::test]
    async fn live_planned_executor_reports_capabilities_but_skips_stage_execution() {
        let executor = LiveRuntimeExecutor::from_gate(LiveExecutorGateReport {
            live_network_authorized: true,
            device_state_changes_authorized: true,
            adb_path_configured: true,
            device_admin_url_configured: true,
            implementation_ready: false,
            effective_live_network_allowed: true,
            effective_device_state_changes_allowed: false,
            blockers: vec![
                "device_state_change_executor_not_implemented",
                "live_runtime_partially_implemented",
            ],
            sensitive_values_policy: "presence_flags_only_no_paths_or_urls_serialized",
        });
        let report = executor.describe();

        assert_eq!(report.executor_id, "live_runtime_executor");
        assert_eq!(report.mode, "live_planned");
        assert!(report.live_gate.live_network_authorized);
        assert!(report
            .capabilities
            .iter()
            .any(|capability| capability.stage == "epdg" && capability.enabled));
        assert!(report
            .capabilities
            .iter()
            .any(|capability| !capability.enabled));
        assert!(report.capabilities.iter().any(|capability| {
            capability.stage == "ike" && capability.reason == "live_stage_available"
        }));

        let result = executor
            .run_stage(ExecutorStageRequest {
                stage: ExecutorStage::SimAuth,
                profile_id: Some("gb_ee_23433".to_string()),
                plmn: Some("23433".to_string()),
                trace_id: "live-planned".to_string(),
            })
            .await;

        assert_eq!(result.status, "skipped");
        assert_eq!(
            result.reason.as_deref(),
            Some("device_state_change_executor_disabled")
        );
        assert_eq!(
            result
                .soak_observation
                .as_ref()
                .map(|observation| observation.scenario_id),
            Some("esim_restore_race_soak")
        );
    }

    #[test]
    fn serialized_report_has_no_private_identity_or_key_material_fields() {
        let report = NoopRuntimeExecutor.describe();
        let json = serde_json::to_string(&report).expect("serialize executor report");

        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "eid",
            "spi",
            "ck",
            "ik",
            "nonce",
            "authorization",
            "password",
            "token",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "executor report must not contain a {forbidden_key} field"
            );
        }
    }
}
