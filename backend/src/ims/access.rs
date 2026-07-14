//! Closed-set IMS access-leg and protected-channel abstractions.
//!
//! The project has two IMS legs (VoWiFi and VoLTE), so runtime dispatch uses
//! enums rather than trait objects. The traits below provide the common
//! contract; the enums retain exhaustive, allocation-free dispatch.

use std::{net::IpAddr, time::Duration};

use super::{context::ImsRoute, ImsError};

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
    use crate::ims::context::SipTransport;
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
}
