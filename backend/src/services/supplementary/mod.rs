//! Per-line supplementary-service runtime state.
//!
//! Network-authoritative Ut rules will be keyed by SIM binding in their store;
//! this object owns only volatile subscription/readiness state for one line.

pub mod ut;

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::RwLock;

use crate::connectivity::core::registration::ImsRegistrationAccess;
use crate::connectivity::core::supplementary::{
    CapabilityReadiness, MessageWaitingSummary, NetworkToggleState,
};
use crate::connectivity::core::ut::{UtDocument, UtDocumentKind};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SupplementarySnapshot {
    pub line_id: String,
    pub call_waiting: NetworkToggleState,
    pub call_waiting_capability: CapabilityReadiness,
    pub forwarding_capability: CapabilityReadiness,
    pub identity_capability: CapabilityReadiness,
    pub mwi_capability: CapabilityReadiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_waiting: Option<MessageWaitingSummary>,
}

impl SupplementarySnapshot {
    fn for_line(line_id: &str) -> Self {
        let not_connected = || CapabilityReadiness::unsupported("supplementary_not_connected");
        Self {
            line_id: line_id.to_string(),
            call_waiting: NetworkToggleState::Unknown,
            call_waiting_capability: not_connected(),
            forwarding_capability: not_connected(),
            identity_capability: not_connected(),
            mwi_capability: not_connected(),
            message_waiting: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupplementaryRuntime {
    line_id: Arc<str>,
    state: Arc<RwLock<SupplementaryState>>,
}

#[derive(Debug)]
struct SupplementaryState {
    snapshot: SupplementarySnapshot,
    mwi_access: Option<ImsRegistrationAccess>,
    ut_access: Option<ImsRegistrationAccess>,
}

impl SupplementaryRuntime {
    pub fn for_line(line_id: impl AsRef<str>) -> Self {
        let line_id = line_id.as_ref().trim();
        assert!(
            !line_id.is_empty(),
            "supplementary runtime requires line_id"
        );
        Self {
            line_id: Arc::from(line_id),
            state: Arc::new(RwLock::new(SupplementaryState {
                snapshot: SupplementarySnapshot::for_line(line_id),
                mwi_access: None,
                ut_access: None,
            })),
        }
    }

    pub fn line_id(&self) -> &str {
        &self.line_id
    }

    pub async fn snapshot(&self) -> SupplementarySnapshot {
        self.state.read().await.snapshot.clone()
    }

    pub async fn begin_mwi_subscription(&self, access: ImsRegistrationAccess) {
        let mut state = self.state.write().await;
        state.mwi_access = Some(access);
        state.snapshot.mwi_capability =
            CapabilityReadiness::supported(false, Some("mwi_subscribe_pending".to_string()));
    }

    pub async fn owns_mwi_subscription(&self, access: ImsRegistrationAccess) -> bool {
        self.state.read().await.mwi_access == Some(access)
    }

    pub async fn mark_mwi_subscribed(&self, access: ImsRegistrationAccess) {
        let mut state = self.state.write().await;
        if state.mwi_access == Some(access) {
            state.snapshot.mwi_capability = CapabilityReadiness::supported(true, None);
        }
    }

    pub async fn fail_mwi_subscription(
        &self,
        access: ImsRegistrationAccess,
        reason: impl Into<String>,
    ) {
        let mut state = self.state.write().await;
        if state.mwi_access == Some(access) {
            state.snapshot.mwi_capability =
                CapabilityReadiness::supported(false, Some(reason.into()));
        }
    }

    pub async fn update_message_waiting(
        &self,
        access: ImsRegistrationAccess,
        summary: MessageWaitingSummary,
    ) {
        let mut state = self.state.write().await;
        if state.mwi_access == Some(access) {
            state.snapshot.mwi_capability = CapabilityReadiness::supported(true, None);
            state.snapshot.message_waiting = Some(summary);
        }
    }

    pub async fn begin_ut_request(&self, access: ImsRegistrationAccess, kind: UtDocumentKind) {
        let mut state = self.state.write().await;
        state.ut_access = Some(access);
        *ut_capability_mut(&mut state.snapshot, kind) =
            CapabilityReadiness::supported(false, Some("ut_request_pending".to_string()));
    }

    pub async fn mark_ut_document(&self, access: ImsRegistrationAccess, document: &UtDocument) {
        let mut state = self.state.write().await;
        if state.ut_access != Some(access) {
            return;
        }
        *ut_capability_mut(&mut state.snapshot, document.kind) =
            CapabilityReadiness::supported(true, None);
        if document.kind == UtDocumentKind::CommunicationWaiting {
            state.snapshot.call_waiting = match document.call_waiting {
                Some(true) => NetworkToggleState::Enabled,
                Some(false) => NetworkToggleState::Disabled,
                None => NetworkToggleState::Unknown,
            };
        }
    }

    pub async fn fail_ut_request(
        &self,
        access: ImsRegistrationAccess,
        kind: UtDocumentKind,
        reason: impl Into<String>,
    ) {
        let mut state = self.state.write().await;
        if state.ut_access == Some(access) {
            *ut_capability_mut(&mut state.snapshot, kind) =
                CapabilityReadiness::supported(false, Some(reason.into()));
        }
    }

    /// Clear only the access that still owns the subscription. A late teardown
    /// from the old leg must not erase state already established after an
    /// access handover.
    pub async fn clear_registration(&self, access: ImsRegistrationAccess) {
        let mut state = self.state.write().await;
        if state.mwi_access == Some(access) {
            state.mwi_access = None;
            state.snapshot.mwi_capability =
                CapabilityReadiness::unsupported("supplementary_not_connected");
            state.snapshot.message_waiting = None;
        }
        if state.ut_access == Some(access) {
            state.ut_access = None;
            state.snapshot.call_waiting = NetworkToggleState::Unknown;
            state.snapshot.call_waiting_capability =
                CapabilityReadiness::unsupported("supplementary_not_connected");
            state.snapshot.forwarding_capability =
                CapabilityReadiness::unsupported("supplementary_not_connected");
            state.snapshot.identity_capability =
                CapabilityReadiness::unsupported("supplementary_not_connected");
        }
    }
}

fn ut_capability_mut(
    snapshot: &mut SupplementarySnapshot,
    kind: UtDocumentKind,
) -> &mut CapabilityReadiness {
    match kind {
        UtDocumentKind::CommunicationWaiting => &mut snapshot.call_waiting_capability,
        UtDocumentKind::CommunicationDiversion => &mut snapshot.forwarding_capability,
        UtDocumentKind::OriginatingIdentityPresentation
        | UtDocumentKind::OriginatingIdentityRestriction => &mut snapshot.identity_capability,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::supplementary::{MessageCount, MessageWaitingSummary};

    #[tokio::test]
    async fn two_line_mwi_state_is_isolated() {
        let first = SupplementaryRuntime::for_line("line-a");
        let second = SupplementaryRuntime::for_line("line-b");
        first
            .begin_mwi_subscription(ImsRegistrationAccess::Volte)
            .await;
        first
            .update_message_waiting(
                ImsRegistrationAccess::Volte,
                MessageWaitingSummary {
                    source: crate::connectivity::core::supplementary::VoicemailSource::OperatorIms,
                    messages_waiting: true,
                    message_account: Some("sip:mailbox-a@example".to_string()),
                    voice: Some(MessageCount {
                        new: 2,
                        ..Default::default()
                    }),
                },
            )
            .await;

        assert_eq!(
            first
                .snapshot()
                .await
                .message_waiting
                .unwrap()
                .voice
                .unwrap()
                .new,
            2
        );
        assert!(second.snapshot().await.message_waiting.is_none());
        second
            .clear_registration(ImsRegistrationAccess::Volte)
            .await;
        assert!(first.snapshot().await.message_waiting.is_some());
    }

    #[tokio::test]
    async fn stale_access_teardown_does_not_clear_handover_state() {
        let runtime = SupplementaryRuntime::for_line("line-a");
        runtime
            .begin_mwi_subscription(ImsRegistrationAccess::Volte)
            .await;
        runtime
            .begin_mwi_subscription(ImsRegistrationAccess::Vowifi)
            .await;
        runtime
            .update_message_waiting(
                ImsRegistrationAccess::Vowifi,
                MessageWaitingSummary {
                    messages_waiting: true,
                    ..Default::default()
                },
            )
            .await;

        runtime
            .clear_registration(ImsRegistrationAccess::Volte)
            .await;
        assert!(runtime.snapshot().await.message_waiting.is_some());
    }

    #[tokio::test]
    async fn stale_access_teardown_does_not_clear_ut_handover_state() {
        let runtime = SupplementaryRuntime::for_line("line-a");
        runtime
            .begin_ut_request(
                ImsRegistrationAccess::Volte,
                UtDocumentKind::CommunicationWaiting,
            )
            .await;
        runtime
            .begin_ut_request(
                ImsRegistrationAccess::Vowifi,
                UtDocumentKind::CommunicationWaiting,
            )
            .await;
        let mut document = UtDocument::empty(UtDocumentKind::CommunicationWaiting);
        document.set_call_waiting(true);
        runtime
            .mark_ut_document(ImsRegistrationAccess::Vowifi, &document)
            .await;

        runtime
            .clear_registration(ImsRegistrationAccess::Volte)
            .await;
        let snapshot = runtime.snapshot().await;
        assert_eq!(snapshot.call_waiting, NetworkToggleState::Enabled);
        assert!(snapshot.call_waiting_capability.ready);
    }
}
