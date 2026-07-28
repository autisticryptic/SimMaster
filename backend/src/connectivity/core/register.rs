//! Transport-independent IMS REGISTER exchange.
//!
//! The channel owns TCP/UDP/IPsec/ESP details. The authenticator owns USIM AKA
//! and construction of the next authenticated REGISTER. This driver only
//! enforces the SIP transaction sequence and bounded challenge rounds.

use std::time::Duration;

use super::{access::ImsChannel, sip_frame, ImsError};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_AUTH_ROUNDS: u8 = 2;

pub trait RegisterAuthenticator<C>: Send
where
    C: ImsChannel,
{
    /// Prepare the protected channel selected by the challenge.
    ///
    /// VoLTE normally keeps the same xfrm-protected socket, while VoWiFi may
    /// replace its initial TCP transport with a new socket bound to the
    /// Security-Server negotiated ports. The default keeps the current
    /// channel, so simple access legs do not need special handling.
    async fn prepare_authenticated_channel(
        &mut self,
        _challenge_response: &[u8],
        _channel: &mut C,
    ) -> Result<(), ImsError> {
        Ok(())
    }

    async fn authenticated_request(
        &mut self,
        challenge_response: &[u8],
        cseq: u32,
    ) -> Result<Vec<u8>, ImsError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterResult {
    pub response: Vec<u8>,
    pub authenticated: bool,
    pub auth_rounds: u8,
}

pub async fn run_register<C, A>(
    channel: &mut C,
    initial_request: &[u8],
    authenticator: &mut A,
) -> Result<RegisterResult, ImsError>
where
    C: ImsChannel,
    A: RegisterAuthenticator<C>,
{
    channel
        .send_sip(initial_request)
        .await
        .map_err(|_| ImsError::new("ims_register_initial_send_failed"))?;
    let mut response = channel
        .recv_sip(REGISTER_TIMEOUT)
        .await
        .map_err(|_| ImsError::new("ims_register_initial_receive_failed"))?;
    let mut auth_rounds = 0u8;

    loop {
        match sip_frame::parse_status(&response)? {
            200..=299 => {
                return Ok(RegisterResult {
                    response,
                    authenticated: auth_rounds > 0,
                    auth_rounds,
                })
            }
            401 | 407 if auth_rounds < MAX_AUTH_ROUNDS => {
                auth_rounds += 1;
                authenticator
                    .prepare_authenticated_channel(&response, channel)
                    .await?;
                let request = authenticator
                    .authenticated_request(&response, u32::from(auth_rounds) + 1)
                    .await?;
                channel
                    .send_sip(&request)
                    .await
                    .map_err(|_| ImsError::new("ims_register_authenticated_send_failed"))?;
                response = channel
                    .recv_sip(REGISTER_TIMEOUT)
                    .await
                    .map_err(|_| ImsError::new("ims_register_authenticated_receive_failed"))?;
            }
            401 | 407 => return Err(ImsError::new("ims_register_auth_rejected")),
            _ if auth_rounds == 0 => {
                return Err(ImsError::new("ims_register_initial_unexpected_status"))
            }
            _ => {
                return Err(ImsError::new(
                    "ims_register_authenticated_unexpected_status",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
    };
    use std::{
        collections::VecDeque,
        net::{Ipv4Addr, SocketAddr},
    };

    struct FakeChannel {
        responses: VecDeque<Vec<u8>>,
        sends: Vec<Vec<u8>>,
        transport: SipTransport,
    }

    impl ImsChannel for FakeChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sends.push(frame.to_vec());
            Ok(())
        }
        async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            self.responses
                .pop_front()
                .ok_or(ImsError::new("ims_test_response_missing"))
        }
        fn route(&self) -> ImsRoute {
            ImsRoute {
                local_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 5060)),
                pcscf_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 5060)),
                transport: self.transport,
            }
        }
        fn security_verify(&self) -> Option<&str> {
            None
        }
    }

    struct FakeAuthenticator;
    impl RegisterAuthenticator<FakeChannel> for FakeAuthenticator {
        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(format!("REGISTER sip:ims.example SIP/2.0\r\nCSeq: {cseq} REGISTER\r\nContent-Length: 0\r\n\r\n").into_bytes())
        }
    }

    struct SwitchingAuthenticator;

    impl RegisterAuthenticator<FakeChannel> for SwitchingAuthenticator {
        async fn prepare_authenticated_channel(
            &mut self,
            _challenge_response: &[u8],
            channel: &mut FakeChannel,
        ) -> Result<(), ImsError> {
            channel.transport = SipTransport::Tcp;
            Ok(())
        }

        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(format!("REGISTER protected CSeq {cseq}").into_bytes())
        }
    }

    fn response(code: u16, reason: &str) -> Vec<u8> {
        format!("SIP/2.0 {code} {reason}\r\nContent-Length: 0\r\n\r\n").into_bytes()
    }

    #[tokio::test]
    async fn initial_challenge_authenticated_success() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized"), response(200, "OK")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;
        let result = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap();
        assert!(result.authenticated);
        assert_eq!(result.auth_rounds, 1);
        assert_eq!(channel.sends.len(), 2);
        assert!(String::from_utf8_lossy(&channel.sends[1]).contains("CSeq: 2 REGISTER"));
    }

    #[tokio::test]
    async fn repeated_challenge_is_bounded() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response(401, "Unauthorized"),
                response(401, "Unauthorized"),
                response(401, "Unauthorized"),
            ]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;
        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "ims_register_auth_rejected");
        assert_eq!(channel.sends.len(), 3);
    }

    #[tokio::test]
    async fn challenge_can_replace_or_reconfigure_the_protected_channel() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized"), response(200, "OK")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = SwitchingAuthenticator;

        run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap();

        assert_eq!(channel.transport, SipTransport::Tcp);
        assert_eq!(channel.sends[1], b"REGISTER protected CSeq 2");
    }

    #[tokio::test]
    async fn initial_receive_failure_has_initial_stage_code() {
        let mut channel = FakeChannel {
            responses: VecDeque::new(),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_initial_receive_failed");
        assert_eq!(channel.sends.len(), 1);
    }

    #[tokio::test]
    async fn authenticated_receive_failure_has_authenticated_stage_code() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_authenticated_receive_failed");
        assert_eq!(channel.sends.len(), 2);
    }
}
