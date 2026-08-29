//! Closed-set IMS access-leg and protected-channel abstractions.
//!
//! The project has two IMS legs (VoWiFi and VoLTE), so runtime dispatch uses
//! enums rather than trait objects. The traits below provide the common
//! contract; the enums retain exhaustive, allocation-free dispatch.

use std::{collections::VecDeque, net::IpAddr, time::Duration};

use super::{context::ImsRoute, ImsError};

/// A channel-local queue for complete SIP frames temporarily set aside by a
/// REGISTER transaction. The queue is deliberately bounded independently of
/// the per-transaction ignored-frame limit: a long-running session may execute
/// many REGISTER/refresh transactions before a stalled consumer recovers.
pub(crate) const MAX_REQUEUED_SIP_FRAMES: usize = 64;
pub(crate) const MAX_REQUEUED_SIP_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
pub(crate) struct ImsRequeue {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl ImsRequeue {
    pub(crate) fn pop_front(&mut self) -> Option<Vec<u8>> {
        let frame = self.frames.pop_front()?;
        self.bytes = self.bytes.saturating_sub(frame.len());
        Some(frame)
    }

    /// Append a frame without evicting older traffic. Returning `false` means
    /// the newest frame was dropped because retaining it would exceed either
    /// the frame-count or byte budget.
    pub(crate) fn push_back(&mut self, frame: Vec<u8>) -> bool {
        if self.frames.len() >= MAX_REQUEUED_SIP_FRAMES
            || self.bytes.saturating_add(frame.len()) > MAX_REQUEUED_SIP_BYTES
        {
            return false;
        }
        self.bytes += frame.len();
        self.frames.push_back(frame);
        true
    }

    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn into_frames(self) -> VecDeque<Vec<u8>> {
        self.frames
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLegKind {
    Vowifi,
    Volte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegReadiness {
    Disabled,
    Available,
    Establishing,
    Registered,
    Degraded,
}

impl LegReadiness {
    pub const fn can_send(self) -> bool {
        matches!(self, Self::Registered)
    }
}

/// A protected SIP byte channel. Implementations own transport buffering;
/// shared REGISTER/MESSAGE logic only sees complete SIP frames.
pub trait ImsChannel: Send {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError>;
    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError>;
    /// Read the next complete SIP frame from the transport itself, ignoring
    /// any frames previously handed back through [`ImsChannel::requeue`].
    ///
    /// A REGISTER transaction must keep waiting for its own response while
    /// unrelated traffic (NOTIFY/MESSAGE/MWI) shares the same IMS signaling
    /// path; those frames are requeued for the session loop. Reading them
    /// again inside the same transaction loop would spin forever, so the
    /// transaction reader uses this fresh-read path. Channels without a
    /// requeue queue can rely on the default.
    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.recv_sip(timeout).await
    }
    /// Hand a complete SIP frame back to the channel so a later reader (for
    /// example the session loop after REGISTER completes) can process it.
    /// The default drops the frame, which is the safe behavior for adapters
    /// without a side queue.
    fn requeue(&mut self, _frame: Vec<u8>) {}
    fn route(&self) -> ImsRoute;
    fn security_verify(&self) -> Option<&str>;
}

/// Common behavior of an access leg. This trait is used with static dispatch;
/// [`AccessLeg`] is the runtime container.
pub trait ImsAccessLegBehavior {
    type Channel: ImsChannel;

    fn kind(&self) -> AccessLegKind;
    async fn establish(&mut self) -> Result<Self::Channel, ImsError>;
    fn readiness(&self) -> LegReadiness;
    fn pcscf(&self) -> Option<IpAddr>;
    fn local_addr(&self) -> Option<IpAddr>;
    async fn teardown(&mut self);
}

/// Closed-set runtime access-leg dispatcher.
pub enum AccessLeg<Vowifi, Volte> {
    Vowifi(Vowifi),
    Volte(Volte),
}

/// Closed-set protected-channel dispatcher.
pub enum AccessLegChannel<Vowifi, Volte> {
    Vowifi(Vowifi),
    Volte(Volte),
}

impl<Vowifi, Volte> AccessLeg<Vowifi, Volte>
where
    Vowifi: ImsAccessLegBehavior,
    Volte: ImsAccessLegBehavior,
{
    pub fn kind(&self) -> AccessLegKind {
        match self {
            Self::Vowifi(leg) => leg.kind(),
            Self::Volte(leg) => leg.kind(),
        }
    }

    pub fn readiness(&self) -> LegReadiness {
        match self {
            Self::Vowifi(leg) => leg.readiness(),
            Self::Volte(leg) => leg.readiness(),
        }
    }

    pub async fn establish(
        &mut self,
    ) -> Result<AccessLegChannel<Vowifi::Channel, Volte::Channel>, ImsError> {
        match self {
            Self::Vowifi(leg) => leg.establish().await.map(AccessLegChannel::Vowifi),
            Self::Volte(leg) => leg.establish().await.map(AccessLegChannel::Volte),
        }
    }

    pub async fn teardown(&mut self) {
        match self {
            Self::Vowifi(leg) => leg.teardown().await,
            Self::Volte(leg) => leg.teardown().await,
        }
    }
}

impl<Vowifi, Volte> ImsChannel for AccessLegChannel<Vowifi, Volte>
where
    Vowifi: ImsChannel,
    Volte: ImsChannel,
{
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        match self {
            Self::Vowifi(channel) => channel.send_sip(frame).await,
            Self::Volte(channel) => channel.send_sip(frame).await,
        }
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        match self {
            Self::Vowifi(channel) => channel.recv_sip(timeout).await,
            Self::Volte(channel) => channel.recv_sip(timeout).await,
        }
    }

    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        match self {
            Self::Vowifi(channel) => channel.recv_sip_fresh(timeout).await,
            Self::Volte(channel) => channel.recv_sip_fresh(timeout).await,
        }
    }

    fn requeue(&mut self, frame: Vec<u8>) {
        match self {
            Self::Vowifi(channel) => channel.requeue(frame),
            Self::Volte(channel) => channel.requeue(frame),
        }
    }

    fn route(&self) -> ImsRoute {
        match self {
            Self::Vowifi(channel) => channel.route(),
            Self::Volte(channel) => channel.route(),
        }
    }

    fn security_verify(&self) -> Option<&str> {
        match self {
            Self::Vowifi(channel) => channel.security_verify(),
            Self::Volte(channel) => channel.security_verify(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::context::SipTransport;
    use std::net::{Ipv4Addr, SocketAddr};

    struct FakeChannel {
        route: ImsRoute,
        sent: Vec<u8>,
    }

    impl ImsChannel for FakeChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sent.extend_from_slice(frame);
            Ok(())
        }

        async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            Ok(b"SIP/2.0 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec())
        }

        fn route(&self) -> ImsRoute {
            self.route
        }

        fn security_verify(&self) -> Option<&str> {
            None
        }
    }

    fn fake() -> FakeChannel {
        FakeChannel {
            route: ImsRoute {
                local_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 5060)),
                pcscf_addr: SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 5060)),
                transport: SipTransport::Udp,
            },
            sent: Vec::new(),
        }
    }

    #[tokio::test]
    async fn channel_enum_dispatches_without_trait_objects() {
        let mut channel: AccessLegChannel<FakeChannel, FakeChannel> =
            AccessLegChannel::Vowifi(fake());
        channel.send_sip(b"OPTIONS").await.unwrap();
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap()[..7],
            *b"SIP/2.0"
        );
        assert_eq!(channel.route().transport, SipTransport::Udp);
    }

    #[test]
    fn requeue_is_fifo_and_releases_its_byte_budget_when_drained() {
        let mut queue = ImsRequeue::default();
        assert!(queue.push_back(b"first".to_vec()));
        assert!(queue.push_back(b"second".to_vec()));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.bytes(), 11);
        assert_eq!(queue.pop_front().as_deref(), Some(b"first".as_slice()));
        assert_eq!(queue.bytes(), 6);
        assert_eq!(queue.pop_front().as_deref(), Some(b"second".as_slice()));
        assert_eq!(queue.bytes(), 0);
    }

    #[test]
    fn requeue_drops_the_newest_frame_at_count_and_byte_limits() {
        let mut queue = ImsRequeue::default();
        for index in 0..MAX_REQUEUED_SIP_FRAMES {
            assert!(queue.push_back(vec![index as u8]));
        }
        assert!(!queue.push_back(b"overflow".to_vec()));
        assert_eq!(queue.len(), MAX_REQUEUED_SIP_FRAMES);
        assert_eq!(queue.pop_front(), Some(vec![0]));

        let mut byte_limited = ImsRequeue::default();
        assert!(byte_limited.push_back(vec![0; MAX_REQUEUED_SIP_BYTES]));
        assert!(!byte_limited.push_back(vec![1]));
        assert_eq!(byte_limited.bytes(), MAX_REQUEUED_SIP_BYTES);
    }
}
