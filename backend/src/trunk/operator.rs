//! Per-line non-blocking seam between the Asterisk trunk task and VoLTE live IO.

use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use tokio::sync::broadcast;

use super::bridge::{OperatorCommand, OperatorEvent};
use crate::infra::config::{TrunkIncomingMode, TrunkIpConnectMode};

#[derive(Clone)]
pub struct OperatorLink {
    inner: Arc<OperatorLinkInner>,
}

struct OperatorLinkInner {
    ready: AtomicBool,
    trunk_local_ip: RwLock<Option<IpAddr>>,
    incoming_mode: RwLock<TrunkIncomingMode>,
    ip_connect_mode: RwLock<TrunkIpConnectMode>,
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
                trunk_local_ip: RwLock::new(None),
                incoming_mode: RwLock::new(TrunkIncomingMode::default()),
                ip_connect_mode: RwLock::new(TrunkIpConnectMode::default()),
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

    pub fn set_ip_connect_mode(&self, mode: TrunkIpConnectMode) {
        if let Ok(mut current) = self.inner.ip_connect_mode.write() {
            *current = mode;
        }
    }

    pub fn ip_connect_mode(&self) -> TrunkIpConnectMode {
        self.inner
            .ip_connect_mode
            .read()
            .map(|mode| *mode)
            .unwrap_or_default()
    }

    /// Publish the address selected by the connected Asterisk UDP socket. The
    /// IMS task uses it to bind the internal side of an MT-call RTP relay
    /// before it asks the trunk task to originate the INVITE toward Asterisk.
    pub fn set_trunk_local_ip(&self, address: Option<IpAddr>) {
        if let Ok(mut current) = self.inner.trunk_local_ip.write() {
            *current = address;
        }
    }

    pub fn trunk_local_ip(&self) -> Option<IpAddr> {
        self.inner
            .trunk_local_ip
            .read()
            .ok()
            .and_then(|address| *address)
    }

    pub fn set_incoming_mode(&self, mode: TrunkIncomingMode) {
        if let Ok(mut current) = self.inner.incoming_mode.write() {
            *current = mode;
        }
    }

    pub fn incoming_mode(&self) -> TrunkIncomingMode {
        self.inner
            .incoming_mode
            .read()
            .map(|mode| *mode)
            .unwrap_or_default()
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

    #[test]
    fn shares_selected_trunk_media_address_with_ims_task() {
        let link = OperatorLink::default();
        let address = "192.0.2.10".parse().unwrap();
        assert_eq!(link.trunk_local_ip(), None);
        link.set_trunk_local_ip(Some(address));
        assert_eq!(link.trunk_local_ip(), Some(address));
        link.set_trunk_local_ip(None);
        assert_eq!(link.trunk_local_ip(), None);
    }

    #[test]
    fn shares_incoming_route_mode_with_ims_task() {
        let link = OperatorLink::default();
        assert_eq!(link.incoming_mode(), TrunkIncomingMode::BoundPending);
        link.set_incoming_mode(TrunkIncomingMode::BoundImmediate);
        assert_eq!(link.incoming_mode(), TrunkIncomingMode::BoundImmediate);
    }

    #[test]
    fn shares_ip_connect_mode_with_ims_task() {
        let link = OperatorLink::default();
        assert_eq!(link.ip_connect_mode(), TrunkIpConnectMode::GsmAnswer);
        link.set_ip_connect_mode(TrunkIpConnectMode::FirstRtp);
        assert_eq!(link.ip_connect_mode(), TrunkIpConnectMode::FirstRtp);
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
