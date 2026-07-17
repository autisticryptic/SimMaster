//! Per-line non-blocking seam between the Asterisk trunk task and VoLTE live IO.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::broadcast;

use super::bridge::{OperatorCommand, OperatorEvent};

#[derive(Clone)]
pub struct OperatorLink {
    inner: Arc<OperatorLinkInner>,
}

struct OperatorLinkInner {
    ready: AtomicBool,
    commands: broadcast::Sender<OperatorCommand>,
    events: broadcast::Sender<OperatorEvent>,
}

impl Default for OperatorLink {
    fn default() -> Self {
        let (commands, _) = broadcast::channel(32);
        let (events, _) = broadcast::channel(32);
        Self {
            inner: Arc::new(OperatorLinkInner {
                ready: AtomicBool::new(false),
                commands,
                events,
            }),
        }
    }
}

impl OperatorLink {
    pub fn set_ready(&self, ready: bool) {
        self.inner.ready.store(ready, Ordering::SeqCst);
    }

    pub fn is_available(&self) -> bool {
        self.inner.ready.load(Ordering::SeqCst) && self.inner.commands.receiver_count() > 0
    }

    pub fn subscribe_commands(&self) -> broadcast::Receiver<OperatorCommand> {
        self.inner.commands.subscribe()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<OperatorEvent> {
        self.inner.events.subscribe()
    }

    pub fn send_command(&self, command: OperatorCommand) -> Result<(), Box<OperatorCommand>> {
        self.inner
            .commands
            .send(command)
            .map(|_| ())
            .map_err(|error| Box::new(error.0))
    }

    pub fn send_event(&self, event: OperatorEvent) {
        let _ = self.inner.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_a_live_command_consumer() {
        let link = OperatorLink::default();
        link.set_ready(true);
        assert!(!link.is_available());
        let _commands = link.subscribe_commands();
        assert!(link.is_available());
        link.set_ready(false);
        assert!(!link.is_available());
    }

    #[tokio::test]
    async fn commands_and_events_cross_the_per_line_seam() {
        let link = OperatorLink::default();
        let mut commands = link.subscribe_commands();
        let mut events = link.subscribe_events();
        link.send_command(OperatorCommand::CancelCall {
            call_id: "call-a".into(),
        })
        .unwrap();
        assert!(matches!(
            commands.recv().await.unwrap(),
            OperatorCommand::CancelCall { .. }
        ));
        link.send_event(OperatorEvent::Ended {
            call_id: "call-a".into(),
        });
        assert!(matches!(
            events.recv().await.unwrap(),
            OperatorEvent::Ended { .. }
        ));
    }
}
