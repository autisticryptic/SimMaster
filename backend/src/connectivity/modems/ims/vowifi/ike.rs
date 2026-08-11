#![allow(dead_code)]

use serde::Serialize;

use super::profiles::CarrierProfile;

pub const CLEAN_ROOM_EXCHANGE_PLAN: &[&str] = &[
    "ike_sa_init",
    "ike_auth_eap",
    "usim_aka_challenge",
    "child_sa_install",
    "userspace_esp_ready",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkeExchangePhase {
    Idle,
    IkeSaInit,
    IkeAuthEap,
    UsimAkaPending,
    ChildSaInstall,
    Established,
    Rekey,
    Failed,
}

impl IkeExchangePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            IkeExchangePhase::Idle => "idle",
            IkeExchangePhase::IkeSaInit => "ike_sa_init",
            IkeExchangePhase::IkeAuthEap => "ike_auth_eap",
            IkeExchangePhase::UsimAkaPending => "usim_aka_pending",
            IkeExchangePhase::ChildSaInstall => "child_sa_install",
            IkeExchangePhase::Established => "established",
            IkeExchangePhase::Rekey => "rekey",
            IkeExchangePhase::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AkaChallengeMode {
    Standard,
    ResyncCapable,
}

impl AkaChallengeMode {
    pub fn from_profile_value(value: &str) -> Self {
        match value {
            "resync_capable" => Self::ResyncCapable,
            _ => Self::Standard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ResyncCapable => "resync_capable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub prf: &'static str,
    pub dh_group: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub mode: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildSaPlan {
    pub mode: &'static str,
    pub anti_replay_window: u16,
    pub mtu_strategy: &'static str,
    pub esp_proposals: Vec<EspProposalPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeSessionPlan {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub exchange_phases: &'static [&'static str],
    pub aka_challenge_mode: &'static str,
    pub nat_keepalive_seconds: u16,
    pub dpd_interval_seconds: u16,
    pub reauth_interval_seconds: Option<u16>,
    pub retransmit_policy: &'static str,
    pub mobike_policy: &'static str,
    pub ike_proposals: Vec<IkeProposalPlan>,
    pub child_sa: ChildSaPlan,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeRuntimeState {
    pub phase: &'static str,
    pub session_trace_id: String,
    pub selected_ike_proposal: Option<String>,
    pub selected_esp_proposal: Option<String>,
    pub retransmit_count: u8,
    pub dpd_pending: bool,
    pub last_error: Option<String>,
}

impl Default for IkeRuntimeState {
    fn default() -> Self {
        Self {
            phase: IkeExchangePhase::Idle.as_str(),
            session_trace_id: String::new(),
            selected_ike_proposal: None,
            selected_esp_proposal: None,
            retransmit_count: 0,
            dpd_pending: false,
            last_error: None,
        }
    }
}

pub fn build_session_plan(profile: &'static CarrierProfile) -> IkeSessionPlan {
    IkeSessionPlan {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        exchange_phases: CLEAN_ROOM_EXCHANGE_PLAN,
        aka_challenge_mode: AkaChallengeMode::from_profile_value(profile.ikev2.aka_challenge_mode)
            .as_str(),
        nat_keepalive_seconds: profile.ikev2.nat_keepalive_seconds,
        dpd_interval_seconds: profile.ikev2.dpd_interval_seconds,
        reauth_interval_seconds: profile.ikev2.reauth_interval_seconds,
        retransmit_policy: "exponential_backoff_with_exchange_deadline",
        mobike_policy: "disabled_until_ip_drift_tests",
        ike_proposals: profile
            .ikev2
            .ike_proposals
            .iter()
            .map(|proposal| parse_ike_proposal(proposal))
            .collect(),
        child_sa: ChildSaPlan {
            mode: "tunnel",
            anti_replay_window: 64,
            mtu_strategy: "inner_path_mtu_with_ipv4_udp_esp_overhead",
            esp_proposals: profile
                .ikev2
                .esp_proposals
                .iter()
                .map(|proposal| parse_esp_proposal(proposal))
                .collect(),
        },
        sensitive_values_policy: "opaque_runtime_only",
    }
}

fn parse_ike_proposal(proposal: &'static str) -> IkeProposalPlan {
    let tokens = proposal.split('-').collect::<Vec<_>>();
    IkeProposalPlan {
        proposal,
        encryption: first_token(&tokens, |token| token.starts_with("aes")).unwrap_or("unknown"),
        integrity: first_token(&tokens, |token| {
            token.starts_with("sha") || token.starts_with("hmac") || token.starts_with("md5")
        })
        .unwrap_or("profile_default"),
        prf: first_token(&tokens, |token| token.starts_with("prf"))
            .map(|token| token.trim_start_matches("prf"))
            .unwrap_or("profile_default"),
        dh_group: first_token(&tokens, |token| {
            token.starts_with("modp") || token.starts_with("ecp")
        })
        .unwrap_or("profile_default"),
    }
}

fn parse_esp_proposal(proposal: &'static str) -> EspProposalPlan {
    let tokens = proposal.split('-').collect::<Vec<_>>();
    EspProposalPlan {
        proposal,
        encryption: first_token(&tokens, |token| token.starts_with("aes")).unwrap_or("unknown"),
        integrity: first_token(&tokens, |token| {
            token.starts_with("sha") || token.starts_with("hmac") || token.starts_with("md5")
        })
        .unwrap_or("profile_default"),
        mode: "tunnel",
    }
}

fn first_token(tokens: &[&'static str], predicate: impl Fn(&str) -> bool) -> Option<&'static str> {
    tokens.iter().copied().find(|token| predicate(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::profiles::{GB_EE_23433, NL_VODAFONE_20404};

    #[test]
    fn builds_gb_ee_plan_from_profile_policy() {
        let plan = build_session_plan(&GB_EE_23433);

        assert_eq!(plan.profile_id, "gb_ee_23433");
        assert_eq!(plan.plmn, "23433");
        assert_eq!(plan.nat_keepalive_seconds, 20);
        assert_eq!(plan.dpd_interval_seconds, 600);
        assert_eq!(plan.aka_challenge_mode, "standard");
        assert_eq!(plan.ike_proposals[0].encryption, "aes128");
        assert_eq!(plan.ike_proposals[0].integrity, "sha256");
        assert_eq!(plan.ike_proposals[0].dh_group, "modp2048");
        assert_eq!(plan.child_sa.esp_proposals[0].proposal, "aes128-sha256");
        assert_eq!(plan.child_sa.anti_replay_window, 64);
    }

    #[test]
    fn parses_explicit_prf_without_losing_two_digit_mnc_profile() {
        let plan = build_session_plan(&NL_VODAFONE_20404);

        assert_eq!(plan.profile_id, "nl_vodafone_20404");
        assert_eq!(plan.plmn, "20404");
        assert_eq!(plan.ike_proposals[0].encryption, "aes256");
        assert_eq!(plan.ike_proposals[0].integrity, "sha256");
        assert_eq!(plan.ike_proposals[0].prf, "sha512");
        assert_eq!(plan.ike_proposals[0].dh_group, "modp2048");
    }

    #[test]
    fn serialized_plan_does_not_expose_private_identifiers_or_material() {
        let plan = build_session_plan(&GB_EE_23433);
        let json = serde_json::to_string(&plan).expect("serialize plan");

        for forbidden_key in [
            "imsi",
            "iccid",
            "eid",
            "msisdn",
            "aka_response",
            "ck",
            "ik",
            "spi",
            "key_material",
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
