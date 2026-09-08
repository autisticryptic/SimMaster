//! Pure voice-leg route planning.
//!
//! The planner selects the IMS access leg that can carry a call and records why
//! another leg was rejected. The per-line Trunk bridge owns SIP and media IO.

use crate::platform::config::{AccessPathKind, VoicePathPolicy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceLegReadiness {
    pub kind: AccessPathKind,
    pub feature_enabled: bool,
    pub registered: bool,
    pub media_gateway_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceRouteRejection {
    PolicyDisabled,
    FeatureDisabled,
    NotRegistered,
    MediaGatewayUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedVoiceLeg {
    pub kind: AccessPathKind,
    pub reason: VoiceRouteRejection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceRoutePlan {
    pub gateway_mode: bool,
    pub candidates: Vec<AccessPathKind>,
    pub rejected: Vec<RejectedVoiceLeg>,
}

pub fn plan_voice_route(
    policy: &VoicePathPolicy,
    readiness: &[VoiceLegReadiness],
) -> VoiceRoutePlan {
    let policy = policy.clone().normalized();
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();

    for layer in policy.priority {
        if !layer.enabled {
            rejected.push(RejectedVoiceLeg {
                kind: layer.kind,
                reason: VoiceRouteRejection::PolicyDisabled,
            });
            continue;
        }
        let Some(state) = readiness.iter().find(|state| state.kind == layer.kind) else {
            rejected.push(RejectedVoiceLeg {
                kind: layer.kind,
                reason: VoiceRouteRejection::FeatureDisabled,
            });
            continue;
        };
        let reason = if !state.feature_enabled {
            Some(VoiceRouteRejection::FeatureDisabled)
        } else if !state.registered {
            Some(VoiceRouteRejection::NotRegistered)
        } else if policy.gateway_mode && !state.media_gateway_ready {
            Some(VoiceRouteRejection::MediaGatewayUnavailable)
        } else {
            None
        };
        if let Some(reason) = reason {
            rejected.push(RejectedVoiceLeg {
                kind: layer.kind,
                reason,
            });
        } else {
            candidates.push(layer.kind);
        }
    }

    // When both IMS legs can actually carry the call, prefer VoWiFi even if a
    // legacy saved order put cellular first. Never enable a disabled leg, and
    // never change the owner of an incoming/already-established dialog.
    candidates.sort_by_key(|kind| match kind {
        AccessPathKind::Vowifi => 0,
        AccessPathKind::CellularIms => 1,
        AccessPathKind::Cs => 2,
    });
    VoiceRoutePlan {
        gateway_mode: policy.gateway_mode,
        candidates,
        rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::config::PathLayerConfig;

    #[test]
    fn dual_ready_calls_prefer_wifi_without_rewriting_saved_policy_or_enabling_it() {
        let mut policy = VoicePathPolicy::default();
        policy.priority.reverse();
        let before = policy.clone();
        let states = [
            state(AccessPathKind::CellularIms, true, true, true),
            state(AccessPathKind::Vowifi, true, true, true),
        ];
        assert_eq!(
            plan_voice_route(&policy, &states).candidates,
            vec![AccessPathKind::Vowifi, AccessPathKind::CellularIms]
        );
        assert_eq!(policy, before);
        policy
            .priority
            .iter_mut()
            .find(|layer| layer.kind == AccessPathKind::Vowifi)
            .unwrap()
            .enabled = false;
        assert_eq!(
            plan_voice_route(&policy, &states).candidates,
            vec![AccessPathKind::CellularIms]
        );
    }

    fn state(
        kind: AccessPathKind,
        feature_enabled: bool,
        registered: bool,
        media_gateway_ready: bool,
    ) -> VoiceLegReadiness {
        VoiceLegReadiness {
            kind,
            feature_enabled,
            registered,
            media_gateway_ready,
        }
    }

    #[test]
    fn preserves_priority_and_requires_gateway_media() {
        let policy = VoicePathPolicy::default();
        let plan = plan_voice_route(
            &policy,
            &[
                state(AccessPathKind::Vowifi, true, true, false),
                state(AccessPathKind::CellularIms, true, true, true),
            ],
        );
        assert_eq!(plan.candidates, vec![AccessPathKind::CellularIms]);
        assert_eq!(
            plan.rejected[0].reason,
            VoiceRouteRejection::MediaGatewayUnavailable
        );
    }

    #[test]
    fn reports_policy_and_feature_rejections() {
        let policy = VoicePathPolicy {
            priority: vec![
                PathLayerConfig {
                    kind: AccessPathKind::Vowifi,
                    enabled: false,
                },
                PathLayerConfig {
                    kind: AccessPathKind::CellularIms,
                    enabled: true,
                },
            ],
            gateway_mode: true,
        };
        let plan = plan_voice_route(
            &policy,
            &[state(AccessPathKind::CellularIms, false, false, false)],
        );
        assert!(plan.candidates.is_empty());
        assert_eq!(plan.rejected[0].reason, VoiceRouteRejection::PolicyDisabled);
        assert_eq!(
            plan.rejected[1].reason,
            VoiceRouteRejection::FeatureDisabled
        );
    }
}
