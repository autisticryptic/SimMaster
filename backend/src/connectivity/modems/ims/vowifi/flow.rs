use serde::Serialize;

use super::diagnostics::VowifiReadiness;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStage {
    NotStarted,
    IdentityReady,
    ProfileMatched,
    SimAuthReady,
    EpdgReady,
    IkeReady,
    ChildSaReady,
    EspReady,
    ImsRegistered,
    SmsReady,
    VoiceReady,
    Degraded,
    Failed,
}

impl RuntimeStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::IdentityReady => "identity_ready",
            Self::ProfileMatched => "profile_matched",
            Self::SimAuthReady => "sim_auth_ready",
            Self::EpdgReady => "epdg_ready",
            Self::IkeReady => "ike_ready",
            Self::ChildSaReady => "child_sa_ready",
            Self::EspReady => "esp_ready",
            Self::ImsRegistered => "ims_registered",
            Self::SmsReady => "sms_ready",
            Self::VoiceReady => "voice_ready",
            Self::Degraded => "degraded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowStepState {
    Waiting,
    Ready,
    Done,
    Blocked,
    Failed,
}

impl FlowStepState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Ready => "ready",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeFlowStep {
    pub id: &'static str,
    pub component: &'static str,
    pub stage: &'static str,
    pub state: &'static str,
    pub readiness_key: &'static str,
    pub blocking_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeFlowStatus {
    pub stage: &'static str,
    pub controlplane_mode: &'static str,
    pub dataplane_mode: &'static str,
    pub steps: Vec<RuntimeFlowStep>,
}

#[derive(Debug, Clone, Copy)]
struct StepDefinition {
    id: &'static str,
    component: &'static str,
    stage: RuntimeStage,
    readiness_key: &'static str,
    ready: fn(&VowifiReadiness) -> bool,
    missing_reason: &'static str,
}

const STEP_DEFINITIONS: &[StepDefinition] = &[
    StepDefinition {
        id: "identity",
        component: "sim_identity",
        stage: RuntimeStage::IdentityReady,
        readiness_key: "identity_ready",
        ready: |readiness| readiness.identity_ready,
        missing_reason: "sim_identity_unavailable",
    },
    StepDefinition {
        id: "profile",
        component: "carrier_profile",
        stage: RuntimeStage::ProfileMatched,
        readiness_key: "profile_matched",
        ready: |readiness| readiness.profile_matched,
        missing_reason: "carrier_profile_not_matched",
    },
    StepDefinition {
        id: "sim_auth",
        component: "usim_aka",
        stage: RuntimeStage::SimAuthReady,
        readiness_key: "sim_auth_ready",
        ready: |readiness| readiness.sim_auth_ready,
        missing_reason: "sim_auth_not_ready",
    },
    StepDefinition {
        id: "epdg",
        component: "epdg_transport",
        stage: RuntimeStage::EpdgReady,
        readiness_key: "epdg_ready",
        ready: |readiness| readiness.epdg_ready,
        missing_reason: "epdg_not_ready",
    },
    StepDefinition {
        id: "ike",
        component: "ikev2_eap_aka",
        stage: RuntimeStage::IkeReady,
        readiness_key: "ike_ready",
        ready: |readiness| readiness.ike_ready,
        missing_reason: "ike_not_ready",
    },
    StepDefinition {
        id: "child_sa",
        component: "child_sa",
        stage: RuntimeStage::ChildSaReady,
        readiness_key: "child_sa_ready",
        ready: |readiness| readiness.child_sa_ready,
        missing_reason: "child_sa_not_ready",
    },
    StepDefinition {
        id: "esp",
        component: "userspace_esp",
        stage: RuntimeStage::EspReady,
        readiness_key: "esp_ready",
        ready: |readiness| readiness.esp_ready,
        missing_reason: "esp_not_ready",
    },
    StepDefinition {
        id: "ims",
        component: "ims_register",
        stage: RuntimeStage::ImsRegistered,
        readiness_key: "ims_registered",
        ready: |readiness| readiness.ims_registered,
        missing_reason: "ims_not_registered",
    },
    StepDefinition {
        id: "sms",
        component: "sms_over_ims",
        stage: RuntimeStage::SmsReady,
        readiness_key: "sms_ready",
        ready: |readiness| readiness.sms_ready,
        missing_reason: "sms_not_ready",
    },
    StepDefinition {
        id: "voice",
        component: "voice_over_ims",
        stage: RuntimeStage::VoiceReady,
        readiness_key: "voice_ready",
        ready: |readiness| readiness.voice_ready,
        missing_reason: "voice_not_ready",
    },
];

pub fn build_runtime_flow(
    readiness: &VowifiReadiness,
    degraded_reason: Option<&str>,
    failed: bool,
) -> RuntimeFlowStatus {
    let terminal_stage = if failed {
        RuntimeStage::Failed
    } else if degraded_reason.is_some() {
        RuntimeStage::Degraded
    } else {
        highest_ready_stage(readiness)
    };

    let mut previous_done = true;
    let mut first_actionable_marked = false;
    let mut steps = Vec::with_capacity(STEP_DEFINITIONS.len());

    for definition in STEP_DEFINITIONS {
        let is_done = (definition.ready)(readiness);
        let state = if failed || degraded_reason.is_some() && !is_done && previous_done {
            FlowStepState::Failed
        } else if is_done {
            FlowStepState::Done
        } else if !previous_done {
            FlowStepState::Waiting
        } else if readiness.profile_matched && !first_actionable_marked {
            first_actionable_marked = true;
            FlowStepState::Ready
        } else if readiness.identity_ready {
            FlowStepState::Blocked
        } else {
            FlowStepState::Waiting
        };

        steps.push(RuntimeFlowStep {
            id: definition.id,
            component: definition.component,
            stage: definition.stage.as_str(),
            state: state.as_str(),
            readiness_key: definition.readiness_key,
            blocking_reason: (!is_done
                && matches!(state, FlowStepState::Blocked | FlowStepState::Failed))
            .then_some(definition.missing_reason),
        });

        previous_done = previous_done && is_done;
    }

    RuntimeFlowStatus {
        stage: terminal_stage.as_str(),
        controlplane_mode: controlplane_mode(terminal_stage, readiness),
        dataplane_mode: dataplane_mode(readiness),
        steps,
    }
}

fn highest_ready_stage(readiness: &VowifiReadiness) -> RuntimeStage {
    if readiness.voice_ready {
        RuntimeStage::VoiceReady
    } else if readiness.sms_ready {
        RuntimeStage::SmsReady
    } else if readiness.ims_registered {
        RuntimeStage::ImsRegistered
    } else if readiness.esp_ready {
        RuntimeStage::EspReady
    } else if readiness.child_sa_ready {
        RuntimeStage::ChildSaReady
    } else if readiness.ike_ready {
        RuntimeStage::IkeReady
    } else if readiness.epdg_ready {
        RuntimeStage::EpdgReady
    } else if readiness.sim_auth_ready {
        RuntimeStage::SimAuthReady
    } else if readiness.profile_matched {
        RuntimeStage::ProfileMatched
    } else if readiness.identity_ready {
        RuntimeStage::IdentityReady
    } else {
        RuntimeStage::NotStarted
    }
}

fn controlplane_mode(stage: RuntimeStage, readiness: &VowifiReadiness) -> &'static str {
    match stage {
        RuntimeStage::Failed => "runtime_failed",
        RuntimeStage::Degraded => "runtime_degraded",
        RuntimeStage::NotStarted => "not_ready",
        RuntimeStage::IdentityReady | RuntimeStage::ProfileMatched => "runtime_idle",
        _ if readiness.ims_registered => "ims_registered",
        _ => "runtime_active",
    }
}

fn dataplane_mode(readiness: &VowifiReadiness) -> &'static str {
    if readiness.esp_ready {
        "userspace_esp"
    } else if readiness.child_sa_ready {
        "child_sa_pending_esp"
    } else if readiness.profile_matched {
        "planned"
    } else {
        "none"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readiness() -> VowifiReadiness {
        VowifiReadiness {
            identity_ready: false,
            sim_auth_ready: false,
            profile_matched: false,
            epdg_ready: false,
            ike_ready: false,
            child_sa_ready: false,
            esp_ready: false,
            ims_registered: false,
            sms_ready: false,
            voice_ready: false,
        }
    }

    #[test]
    fn no_sim_keeps_runtime_not_started() {
        let flow = build_runtime_flow(&readiness(), None, false);

        assert_eq!(flow.stage, "not_started");
        assert_eq!(flow.dataplane_mode, "none");
        assert_eq!(flow.steps[0].state, "waiting");
    }

    #[test]
    fn unmatched_profile_blocks_after_identity() {
        let mut readiness = readiness();
        readiness.identity_ready = true;

        let flow = build_runtime_flow(&readiness, None, false);

        assert_eq!(flow.stage, "identity_ready");
        assert_eq!(flow.steps[0].state, "done");
        assert_eq!(flow.steps[1].state, "blocked");
        assert_eq!(
            flow.steps[1].blocking_reason,
            Some("carrier_profile_not_matched")
        );
    }

    #[test]
    fn matched_profile_marks_next_runtime_step_ready() {
        let mut readiness = readiness();
        readiness.identity_ready = true;
        readiness.profile_matched = true;

        let flow = build_runtime_flow(&readiness, None, false);

        assert_eq!(flow.stage, "profile_matched");
        assert_eq!(flow.controlplane_mode, "runtime_idle");
        assert_eq!(flow.dataplane_mode, "planned");
        assert_eq!(flow.steps[2].id, "sim_auth");
        assert_eq!(flow.steps[2].state, "ready");
    }

    #[test]
    fn full_sms_ready_flow_reports_userspace_esp() {
        let flow = build_runtime_flow(
            &VowifiReadiness {
                identity_ready: true,
                sim_auth_ready: true,
                profile_matched: true,
                epdg_ready: true,
                ike_ready: true,
                child_sa_ready: true,
                esp_ready: true,
                ims_registered: true,
                sms_ready: true,
                voice_ready: true,
            },
            None,
            false,
        );

        assert_eq!(flow.stage, "voice_ready");
        assert_eq!(flow.controlplane_mode, "ims_registered");
        assert_eq!(flow.dataplane_mode, "userspace_esp");
        assert!(flow.steps.iter().all(|step| step.state == "done"));
    }

    #[test]
    fn degraded_flow_marks_first_missing_runtime_stage_failed() {
        let mut readiness = readiness();
        readiness.identity_ready = true;
        readiness.profile_matched = true;

        let flow = build_runtime_flow(&readiness, Some("context_canceled"), false);

        assert_eq!(flow.stage, "degraded");
        assert_eq!(flow.controlplane_mode, "runtime_degraded");
        assert_eq!(flow.steps[2].state, "failed");
    }
}
