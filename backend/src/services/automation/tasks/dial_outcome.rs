//! Dial-task success is not the same as remote answering. Evidence is scoped
//! to one access attempt; raw call history is never edited by this classifier.
use crate::{
    connectivity::core::ims_failure::ImsFailureDiagnostic,
    services::trunk::bridge::{OperatorEvent, VoiceCallObservation},
};
use anyhow::{anyhow, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialOutcome {
    PeerNoAnswer,
    PeerBusy,
    PeerDeclined,
    RemoteEnded,
    RingingAtDeadline,
    AnsweredAtDeadline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialReport {
    pub outcome: DialOutcome,
    pub ringing_observed: bool,
    pub answered_observed: bool,
    pub sip_status: Option<u16>,
    pub q850_cause: Option<u16>,
}
impl DialReport {
    /// Fixed labels and protocol numbers only, no phone/URI/provider text.
    pub fn detail(&self) -> String {
        let (code, label) = match self.outcome {
            DialOutcome::PeerNoAnswer => ("peer_no_answer", "对方未接听／对端无应答超时"),
            DialOutcome::PeerBusy => ("peer_busy", "对端忙"),
            DialOutcome::PeerDeclined => ("peer_declined", "对端拒接"),
            DialOutcome::RemoteEnded => ("remote_ended", "已观察到接听，随后远端结束"),
            DialOutcome::RingingAtDeadline => (
                "ringing_at_deadline",
                "已收到振铃响应，观察时段结束仍未见接听，已请求挂机",
            ),
            DialOutcome::AnsweredAtDeadline => (
                "answered_at_deadline",
                "已观察到接听，观察时段结束，已请求挂机",
            ),
        };
        let mut detail = format!(
            "拨号任务成功：{label}；call_outcome={code}；ringing_observed={}；answered_observed={}",
            self.ringing_observed, self.answered_observed
        );
        if let Some(status) = self.sip_status {
            detail.push_str(&format!("；SIP={status}"));
        }
        if let Some(cause) = self.q850_cause {
            detail.push_str(&format!("；Q.850={cause}"));
        }
        detail.push_str(if self.answered_observed {
            "；不代表音频或通话时长已验收"
        } else {
            "；不表示已接通"
        });
        detail
    }
}

#[derive(Default)]
pub(super) struct DialEvidence {
    ringing: bool,
    answered: bool,
}
impl DialEvidence {
    fn report(
        &self,
        outcome: DialOutcome,
        diagnostic: Option<&ImsFailureDiagnostic>,
    ) -> DialReport {
        DialReport {
            outcome,
            ringing_observed: self.ringing,
            answered_observed: self.answered,
            sip_status: diagnostic.map(|d| d.sip_status),
            q850_cause: diagnostic.and_then(|d| d.q850_cause),
        }
    }

    pub fn event(&mut self, call_id: &str, event: &OperatorEvent) -> Option<Result<DialReport>> {
        match event {
            OperatorEvent::AttemptChanged { call_id: id } if id == call_id => {
                *self = Self::default();
            }
            OperatorEvent::Observation { call_id: id, fact }
                if id == call_id
                    || (id.is_empty() && *fact == VoiceCallObservation::EvidenceLost) =>
            {
                match fact {
                    VoiceCallObservation::RemoteRinging => self.ringing = true,
                    VoiceCallObservation::RemoteAnswered => self.answered = true,
                    VoiceCallObservation::RemoteEnded => {
                        return Some(if self.answered {
                            Ok(self.report(DialOutcome::RemoteEnded, None))
                        } else {
                            Err(anyhow!("automation_call_ended_before_answer"))
                        })
                    }
                    VoiceCallObservation::LocalCancelled => {
                        return Some(Err(anyhow!("automation_call_cancelled")))
                    }
                    VoiceCallObservation::LocalFailure => {
                        return Some(Err(anyhow!("automation_call_local_failure")))
                    }
                    VoiceCallObservation::EvidenceLost => {
                        return Some(Err(anyhow!("automation_call_observation_lost")))
                    }
                }
            }
            OperatorEvent::Rejected {
                call_id: id,
                status,
                diagnostic,
            } if id == call_id => {
                let failure = || Err(anyhow!("automation_call_rejected:sip_status={status}"));
                // Even after ringing, local synthesized 486/408 or a later
                // re-INVITE failure is not a called-party disposition.
                if self.answered
                    || !diagnostic.network_response
                    || !diagnostic.initial_invite
                    || diagnostic.sip_status != *status
                {
                    return Some(failure());
                }
                let outcome = match (*status, diagnostic.q850_cause, diagnostic.code) {
                    (486 | 600, None | Some(17), "callee_busy") => Some(DialOutcome::PeerBusy),
                    (603, None | Some(21), "call_declined" | "call_rejected") => {
                        Some(DialOutcome::PeerDeclined)
                    }
                    (
                        408 | 480,
                        None | Some(18 | 19 | 31),
                        "sip_request_timeout"
                        | "callee_temporarily_unavailable"
                        | "callee_not_responding"
                        | "callee_no_answer"
                        | "normal_unspecified",
                    ) if self.ringing || diagnostic.q850_cause == Some(19) => {
                        Some(DialOutcome::PeerNoAnswer)
                    }
                    _ => None,
                };
                return Some(outcome.map_or_else(failure, |outcome| {
                    Ok(self.report(outcome, Some(diagnostic)))
                }));
            }
            OperatorEvent::Unavailable { call_id: id } if id == call_id => {
                return Some(Err(anyhow!("automation_call_access_unavailable")))
            }
            OperatorEvent::Cancelled { call_id: id } if id == call_id => {
                return Some(Err(anyhow!("automation_call_cancelled")))
            }
            OperatorEvent::Ended { call_id: id } if id == call_id => {
                return Some(Err(anyhow!(
                    "automation_call_ended_without_remote_evidence"
                )))
            }
            // Started/100/183/early-media Answered/Connected are not proof that
            // the called user was alerted or answered an initial INVITE.
            _ => {}
        }
        None
    }

    pub fn deadline(&self) -> Result<DialReport> {
        if self.answered {
            Ok(self.report(DialOutcome::AnsweredAtDeadline, None))
        } else if self.ringing {
            Ok(self.report(DialOutcome::RingingAtDeadline, None))
        } else {
            Err(anyhow!("automation_call_delivery_unconfirmed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn progress(status: u16) -> OperatorEvent {
        if status == 180 {
            OperatorEvent::Observation {
                call_id: "owned".into(),
                fact: VoiceCallObservation::RemoteRinging,
            }
        } else {
            OperatorEvent::Provisional {
                call_id: "owned".into(),
                status,
                body: None,
            }
        }
    }
    fn response(status: u16, q850: Option<u16>, warning: &str) -> OperatorEvent {
        let reason = q850
            .map(|n| format!("Reason: Q.850;cause={n}\r\n"))
            .unwrap_or_default();
        let frame = format!("SIP/2.0 {status} Result\r\nCSeq: 1 INVITE\r\n{reason}{warning}\r\n");
        OperatorEvent::Rejected {
            call_id: "owned".into(),
            status,
            diagnostic: ImsFailureDiagnostic::from_response(frame.as_bytes())
                .unwrap()
                .for_initial_invite(true),
        }
    }
    #[test]
    fn ringing_then_peer_timeout_succeeds_without_claiming_answered() {
        let mut state = DialEvidence::default();
        state.event("owned", &progress(180));
        for status in [408, 480] {
            let report = state
                .event("owned", &response(status, Some(31), ""))
                .unwrap()
                .unwrap();
            assert_eq!(report.outcome, DialOutcome::PeerNoAnswer);
            assert_eq!(report.sip_status, Some(status));
            assert_eq!(report.q850_cause, Some(31));
            assert!(!report.answered_observed);
            assert!(report.detail().contains("不表示已接通"));
        }
    }
    #[test]
    fn bare_timeout_trying_and_early_media_do_not_prove_delivery() {
        for status in [100, 183] {
            let mut state = DialEvidence::default();
            state.event("owned", &progress(status));
            assert!(state.deadline().is_err());
            assert!(state
                .event("owned", &response(408, Some(31), ""))
                .unwrap()
                .is_err());
        }
        assert!(DialEvidence::default()
            .event("owned", &response(408, None, ""))
            .unwrap()
            .is_err());
    }
    #[test]
    fn peer_busy_decline_and_alerted_q850_are_non_caller_outcomes() {
        for (status, cause, expected) in [
            (486, None, DialOutcome::PeerBusy),
            (600, Some(17), DialOutcome::PeerBusy),
            (603, None, DialOutcome::PeerDeclined),
            (603, Some(21), DialOutcome::PeerDeclined),
            (480, Some(19), DialOutcome::PeerNoAnswer),
        ] {
            let report = DialEvidence::default()
                .event("owned", &response(status, cause, ""))
                .unwrap()
                .unwrap();
            assert_eq!(report.outcome, expected);
            assert!(!report.answered_observed);
        }
        assert!(DialEvidence::default()
            .event("owned", &response(480, Some(18), ""))
            .unwrap()
            .is_err());
    }
    #[test]
    fn local_reinvite_policy_and_network_failures_remain_failed_after_ringing() {
        let mut state = DialEvidence::default();
        state.event("owned", &progress(180));
        for cause in [34, 38, 41, 47, 55, 57, 88, 102] {
            assert!(state
                .event("owned", &response(480, Some(cause), ""))
                .unwrap()
                .is_err());
        }
        assert!(state
            .event(
                "owned",
                &response(
                    480,
                    Some(31),
                    "Warning: 399 carrier \"insufficient credit\"\r\n"
                )
            )
            .unwrap()
            .is_err());
        for status in [408, 486] {
            let local = OperatorEvent::Rejected {
                call_id: "owned".into(),
                status,
                diagnostic: ImsFailureDiagnostic::from_status(status).for_initial_invite(true),
            };
            assert!(state.event("owned", &local).unwrap().is_err());
        }
        let mut later = response(408, None, "");
        if let OperatorEvent::Rejected { diagnostic, .. } = &mut later {
            diagnostic.initial_invite = false;
        }
        assert!(state.event("owned", &later).unwrap().is_err());
    }
    #[test]
    fn attempt_change_retires_old_ringing_before_a_new_timeout_or_deadline() {
        let mut state = DialEvidence::default();
        state.event("owned", &progress(180));
        state.event(
            "owned",
            &OperatorEvent::AttemptChanged {
                call_id: "owned".into(),
            },
        );
        assert!(state.deadline().is_err());
        assert!(state
            .event("owned", &response(408, Some(31), ""))
            .unwrap()
            .is_err());
        state.event("owned", &progress(180));
        assert!(state
            .event("owned", &response(408, Some(31), ""))
            .unwrap()
            .is_ok());
    }
    #[test]
    fn early_answer_and_local_end_are_not_remote_answer_or_remote_end() {
        let mut state = DialEvidence::default();
        state.event(
            "owned",
            &OperatorEvent::Answered {
                call_id: "owned".into(),
                body: vec![],
            },
        );
        assert!(state.deadline().is_err());
        state.event(
            "owned",
            &OperatorEvent::Observation {
                call_id: "owned".into(),
                fact: VoiceCallObservation::RemoteAnswered,
            },
        );
        assert_eq!(
            state.deadline().unwrap().outcome,
            DialOutcome::AnsweredAtDeadline
        );
        assert!(state
            .event(
                "owned",
                &OperatorEvent::Ended {
                    call_id: "owned".into()
                }
            )
            .unwrap()
            .is_err());
        assert_eq!(
            state
                .event(
                    "owned",
                    &OperatorEvent::Observation {
                        call_id: "owned".into(),
                        fact: VoiceCallObservation::RemoteEnded
                    }
                )
                .unwrap()
                .unwrap()
                .outcome,
            DialOutcome::RemoteEnded
        );
    }
    #[test]
    fn cancelled_or_lost_evidence_never_turns_into_success() {
        for fact in [
            VoiceCallObservation::LocalCancelled,
            VoiceCallObservation::LocalFailure,
            VoiceCallObservation::EvidenceLost,
        ] {
            let mut state = DialEvidence::default();
            state.event("owned", &progress(180));
            assert!(state
                .event(
                    "owned",
                    &OperatorEvent::Observation {
                        call_id: "owned".into(),
                        fact
                    }
                )
                .unwrap()
                .is_err());
        }
    }
}
