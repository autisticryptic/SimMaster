//! Bounded native-observation wakeups; never carry modem payloads or identities.
//! Events request authoritative reconciliation, not synthesized call/registration
//! success. A lagged subscriber must perform a full reconciliation.
use crate::hardware::cellular::at_urc::UrcEvents;
use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct NativeEvent {
    pub line_id: String,
    pub hints: UrcEvents,
}
static EVENTS: OnceLock<broadcast::Sender<NativeEvent>> = OnceLock::new();
fn sender() -> &'static broadcast::Sender<NativeEvent> {
    EVENTS.get_or_init(|| broadcast::channel(64).0)
}
pub fn subscribe() -> broadcast::Receiver<NativeEvent> {
    sender().subscribe()
}
pub fn publish(line_id: String, hints: UrcEvents) {
    if hints != UrcEvents::default() {
        let _ = sender().send(NativeEvent { line_id, hints });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn subscribers_receive_independent_hints_without_consuming_each_other() {
        let (tx, _) = broadcast::channel(2);
        let (mut calls, mut registration) = (tx.subscribe(), tx.subscribe());
        let hint = NativeEvent {
            line_id: "line-fixture".into(),
            hints: UrcEvents {
                call_changed: true,
                registration_changed: true,
                ..Default::default()
            },
        };
        tx.send(hint).unwrap();
        assert!(calls.recv().await.unwrap().hints.call_changed);
        assert!(
            registration
                .recv()
                .await
                .unwrap()
                .hints
                .registration_changed
        );
    }
    #[tokio::test]
    async fn overflow_is_reported_so_consumers_reconcile_instead_of_assuming_continuity() {
        let (tx, mut rx) = broadcast::channel(2);
        for _ in 0..3 {
            tx.send(NativeEvent {
                line_id: "line-fixture".into(),
                hints: Default::default(),
            })
            .unwrap();
        }
        assert!(matches!(
            rx.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
    }
}
