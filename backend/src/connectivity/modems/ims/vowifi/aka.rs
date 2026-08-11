#![allow(dead_code)]

use serde::Serialize;

use super::profiles::CarrierProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AkaPhase {
    Idle,
    IdentityReady,
    ChannelOpening,
    ChallengePending,
    ChallengeComplete,
    ResyncPending,
    Failed,
}

impl AkaPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::IdentityReady => "identity_ready",
            Self::ChannelOpening => "channel_opening",
            Self::ChallengePending => "challenge_pending",
            Self::ChallengeComplete => "challenge_complete",
            Self::ResyncPending => "resync_pending",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogicalChannelPlan {
    pub application_priority: &'static [&'static str],
    pub channel_scope: &'static str,
    pub open_policy: &'static str,
    pub close_policy: &'static str,
    pub profile_switch_cleanup: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AkaChallengePlan {
    pub method: &'static str,
    pub challenge_source: &'static str,
    pub resync_supported: bool,
    pub failure_mapping: &'static str,
    pub secret_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AkaAdapterPlan {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub identity_source: &'static str,
    pub sim_access: &'static str,
    pub qmi_proxy_policy: &'static str,
    pub logical_channel: LogicalChannelPlan,
    pub challenge: AkaChallengePlan,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AkaRuntimeState {
    pub phase: &'static str,
    pub auth_trace_id: String,
    pub logical_channel_open: bool,
    pub selected_application: Option<String>,
    pub challenge_count: u16,
    pub resync_count: u8,
    pub last_error: Option<String>,
}

impl Default for AkaRuntimeState {
    fn default() -> Self {
        Self {
            phase: AkaPhase::Idle.as_str(),
            auth_trace_id: String::new(),
            logical_channel_open: false,
            selected_application: None,
            challenge_count: 0,
            resync_count: 0,
            last_error: None,
        }
    }
}

pub fn build_adapter_plan(profile: &'static CarrierProfile) -> AkaAdapterPlan {
    let resync_supported = profile.ikev2.aka_challenge_mode == "resync_capable";
    AkaAdapterPlan {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        identity_source: profile.ims.identity_source,
        sim_access: "qmi_uim_first_at_csim_fallback",
        qmi_proxy_policy: "use_device_open_proxy_when_modemmanager_is_present",
        logical_channel: LogicalChannelPlan {
            application_priority: &["isim", "usim"],
            channel_scope: "per_profile_runtime",
            open_policy: "lazy_open_for_eap_aka_challenge",
            close_policy: "close_after_challenge_or_runtime_teardown",
            profile_switch_cleanup: "close_all_owned_channels_before_enable_profile",
        },
        challenge: AkaChallengePlan {
            method: "eap_aka_usim",
            challenge_source: "ikev2_eap_aka",
            resync_supported,
            failure_mapping: "aka_failure_to_ike_notify_and_runtime_degraded_reason",
            secret_values_policy: "rand_autn_res_ck_ik_never_serialized",
        },
        timeout_ms: 8_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::profiles::{GB_EE_23433, US_ATT_310410};

    #[test]
    fn builds_qmi_uim_first_adapter_plan() {
        let plan = build_adapter_plan(&GB_EE_23433);

        assert_eq!(plan.profile_id, "gb_ee_23433");
        assert_eq!(plan.identity_source, "carrier_device_model");
        assert_eq!(plan.sim_access, "qmi_uim_first_at_csim_fallback");
        assert_eq!(plan.logical_channel.application_priority, &["isim", "usim"]);
        assert_eq!(
            plan.logical_channel.profile_switch_cleanup,
            "close_all_owned_channels_before_enable_profile"
        );
        assert_eq!(plan.challenge.method, "eap_aka_usim");
    }

    #[test]
    fn us_profile_keeps_proxy_and_timeout_policy() {
        let plan = build_adapter_plan(&US_ATT_310410);

        assert_eq!(
            plan.qmi_proxy_policy,
            "use_device_open_proxy_when_modemmanager_is_present"
        );
        assert_eq!(plan.timeout_ms, 8_000);
        assert!(!plan.challenge.resync_supported);
    }

    #[test]
    fn serialized_plan_has_no_raw_sim_or_aka_material_fields() {
        let plan = build_adapter_plan(&GB_EE_23433);
        let json = serde_json::to_string(&plan).expect("serialize aka plan");

        for forbidden_key in [
            "imsi", "iccid", "eid", "msisdn", "rand", "autn", "res", "ck", "ik", "auts", "k",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "plan must not contain a {forbidden_key} field"
            );
        }
    }
}
