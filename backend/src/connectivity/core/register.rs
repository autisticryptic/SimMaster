//! Transport-independent IMS REGISTER exchange.
//!
//! The channel owns TCP/UDP/IPsec/ESP details. The authenticator owns USIM AKA
//! and construction of the next authenticated REGISTER. This driver only
//! enforces the SIP transaction sequence and bounded challenge rounds.

use std::time::Duration;

use super::{access::ImsChannel, registration::UnregisterResult, sip_frame, ImsError};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_AUTH_ROUNDS: u8 = 2;
pub(crate) const MAX_REGISTER_PROVISIONAL_RESPONSES: u8 = 4;

pub trait RegisterAuthenticator<C>: Send
where
    C: ImsChannel,
{
    /// Let an access leg own one challenged REGISTER exchange when its security
    /// transport cannot be represented as a single in-place channel update.
    ///
    /// Most legs use the default path below: prepare the existing channel,
    /// build the request, then let this driver send and receive it. VoWiFi is
    /// the important exception because one `Security-Server` offer may require
    /// probing several protected ESP port/SPI mappings. Returning `Some` keeps
    /// that access-specific probing behind the adapter while this driver still
    /// owns the shared 401/407/AUTS transaction state and challenge bound.
    async fn exchange_authenticated(
        &mut self,
        _challenge_response: &[u8],
        _cseq: u32,
        _channel: &mut C,
    ) -> Result<Option<Vec<u8>>, ImsError> {
        Ok(None)
    }

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterFailure {
    pub error: ImsError,
    /// Last complete SIP response seen before the transaction failed. Secret
    /// challenge values remain in this buffer, so callers must only emit
    /// redacted metadata derived from it.
    pub response: Option<Vec<u8>>,
    pub auth_rounds: u8,
}

impl RegisterFailure {
    fn new(error: ImsError, response: Option<Vec<u8>>, auth_rounds: u8) -> Self {
        Self {
            error,
            response,
            auth_rounds,
        }
    }
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
    run_register_observed(channel, initial_request, authenticator)
        .await
        .map_err(|failure| failure.error)
}

/// Run REGISTER while preserving the last complete response on failure.
///
/// This is intended for access-leg diagnostics and bounded interoperability
/// fallbacks. The response may contain nonce/security material and must never
/// be logged or serialized verbatim.
pub async fn run_register_observed<C, A>(
    channel: &mut C,
    initial_request: &[u8],
    authenticator: &mut A,
) -> Result<RegisterResult, RegisterFailure>
where
    C: ImsChannel,
    A: RegisterAuthenticator<C>,
{
    channel.send_sip(initial_request).await.map_err(|_| {
        RegisterFailure::new(ImsError::new("ims_register_initial_send_failed"), None, 0)
    })?;
    let mut response = recv_final_register_response(
        channel,
        "ims_register_initial_receive_failed",
        "ims_register_initial_unexpected_status",
    )
    .await
    .map_err(|error| RegisterFailure::new(error, None, 0))?;
    let mut auth_rounds = 0u8;

    loop {
        let status = sip_frame::parse_status(&response)
            .map_err(|error| RegisterFailure::new(error, Some(response.clone()), auth_rounds))?;
        match status {
            200..=299 => {
                return Ok(RegisterResult {
                    response,
                    authenticated: auth_rounds > 0,
                    auth_rounds,
                })
            }
            401 | 407 if auth_rounds < MAX_AUTH_ROUNDS => {
                auth_rounds += 1;
                let cseq = u32::from(auth_rounds) + 1;
                if let Some(access_response) = authenticator
                    .exchange_authenticated(&response, cseq, channel)
                    .await
                    .map_err(|error| {
                        RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                    })?
                {
                    response = access_response;
                } else {
                    authenticator
                        .prepare_authenticated_channel(&response, channel)
                        .await
                        .map_err(|error| {
                            RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                        })?;
                    let request = authenticator
                        .authenticated_request(&response, cseq)
                        .await
                        .map_err(|error| {
                            RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                        })?;
                    channel.send_sip(&request).await.map_err(|_| {
                        RegisterFailure::new(
                            ImsError::new("ims_register_authenticated_send_failed"),
                            Some(response.clone()),
                            auth_rounds,
                        )
                    })?;
                    response = recv_final_register_response(
                        channel,
                        "ims_register_authenticated_receive_failed",
                        "ims_register_authenticated_unexpected_status",
                    )
                    .await
                    .map_err(|error| RegisterFailure::new(error, None, auth_rounds))?;
                }
            }
            401 | 407 => {
                return Err(RegisterFailure::new(
                    ImsError::new("ims_register_auth_rejected"),
                    Some(response),
                    auth_rounds,
                ))
            }
            _ if auth_rounds == 0 => {
                tracing::warn!(
                    sip_status = status,
                    "IMS REGISTER received unexpected final response"
                );
                return Err(RegisterFailure::new(
                    ImsError::new("ims_register_initial_unexpected_status"),
                    Some(response),
                    auth_rounds,
                ));
            }
            _ => {
                tracing::warn!(
                    sip_status = status,
                    auth_rounds,
                    "IMS REGISTER received unexpected authenticated response"
                );
                return Err(RegisterFailure::new(
                    ImsError::new("ims_register_authenticated_unexpected_status"),
                    Some(response),
                    auth_rounds,
                ));
            }
        }
    }
}

/// Run an explicit `REGISTER Expires: 0` exchange through the same bounded
/// challenge engine as registration and refresh. Only a final 2xx is a
/// network-confirmed unregister; a final non-2xx remains distinct from losing
/// the signaling path before any final response.
pub async fn run_unregister<C, A>(
    channel: &mut C,
    initial_request: &[u8],
    authenticator: &mut A,
) -> UnregisterResult
where
    C: ImsChannel,
    A: RegisterAuthenticator<C>,
{
    match run_register_observed(channel, initial_request, authenticator).await {
        Ok(_) => UnregisterResult::Confirmed,
        Err(failure) if failure.response.is_some() => UnregisterResult::Rejected,
        Err(_) => UnregisterResult::AccessLost,
    }
}

async fn recv_final_register_response<C>(
    channel: &mut C,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    for provisional_count in 0..MAX_REGISTER_PROVISIONAL_RESPONSES {
        let response = channel
            .recv_sip(REGISTER_TIMEOUT)
            .await
            .map_err(|_| ImsError::new(receive_error))?;
        let status = sip_frame::parse_status(&response)?;
        if !(100..=199).contains(&status) {
            return Ok(response);
        }
        tracing::debug!(
            sip_status = status,
            provisional_count = provisional_count + 1,
            "IMS REGISTER provisional response received"
        );
    }
    Err(ImsError::new(provisional_exhausted_error))
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

    struct OwnedExchangeAuthenticator;

    impl RegisterAuthenticator<FakeChannel> for OwnedExchangeAuthenticator {
        async fn exchange_authenticated(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
            channel: &mut FakeChannel,
        ) -> Result<Option<Vec<u8>>, ImsError> {
            channel
                .sends
                .push(format!("REGISTER adapter CSeq {cseq}").into_bytes());
            Ok(Some(response(200, "OK")))
        }

        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            _cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            panic!("adapter-owned exchange must bypass the default request path")
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
    async fn unregister_uses_the_same_bounded_challenge_exchange() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized"), response(200, "OK")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut authenticator = FakeAuthenticator;
        let result = run_unregister(
            &mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nExpires: 0\r\n\r\n",
            &mut authenticator,
        )
        .await;

        assert_eq!(result, UnregisterResult::Confirmed);
        assert_eq!(channel.sends.len(), 2);
        assert!(channel.sends[0]
            .windows(b"Expires: 0".len())
            .any(|window| window == b"Expires: 0"));
    }

    #[tokio::test]
    async fn unregister_does_not_confirm_rejection_or_transport_loss() {
        let mut rejected = FakeChannel {
            responses: VecDeque::from([response(403, "Forbidden")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        assert_eq!(
            run_unregister(&mut rejected, b"unregister", &mut FakeAuthenticator).await,
            UnregisterResult::Rejected
        );

        let mut lost = FakeChannel {
            responses: VecDeque::new(),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        assert_eq!(
            run_unregister(&mut lost, b"unregister", &mut FakeAuthenticator).await,
            UnregisterResult::AccessLost
        );
    }

    #[tokio::test]
    async fn provisional_responses_are_skipped_for_each_register_transaction() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response(100, "Trying"),
                response(401, "Unauthorized"),
                response(100, "Trying"),
                response(200, "OK"),
            ]),
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
        assert!(channel.responses.is_empty());
    }

    #[tokio::test]
    async fn provisional_response_loop_is_bounded() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response(100, "Trying"),
                response(100, "Trying"),
                response(100, "Trying"),
                response(100, "Trying"),
                response(401, "Unauthorized"),
            ]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_initial_unexpected_status");
        assert_eq!(channel.responses.len(), 1);
        assert_eq!(channel.sends.len(), 1);
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
    async fn access_adapter_can_own_a_challenged_exchange() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized")]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = OwnedExchangeAuthenticator;

        let result = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap();

        assert!(result.authenticated);
        assert_eq!(result.auth_rounds, 1);
        assert_eq!(channel.sends[1], b"REGISTER adapter CSeq 2");
        assert!(channel.responses.is_empty());
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
    async fn observed_failure_retains_terminal_response_without_changing_legacy_api() {
        let terminal = b"SIP/2.0 400 Bad Request\r\nWarning: 399 pcscf \"redacted\"\r\nContent-Length: 0\r\n\r\n".to_vec();
        let mut channel = FakeChannel {
            responses: VecDeque::from([terminal.clone()]),
            sends: Vec::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let failure = run_register_observed(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(
            failure.error.code(),
            "ims_register_initial_unexpected_status"
        );
        assert_eq!(failure.response.as_deref(), Some(terminal.as_slice()));
        assert_eq!(failure.auth_rounds, 0);
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

/// Reusable REGISTER contract exercised from both live access adapters. The
/// harness models the two supported exchange shapes without requiring a modem:
/// VoLTE leaves authenticated send/receive to the shared driver, while VoWiFi
/// owns the protected exchange and must return a final (non-provisional) frame.
#[cfg(test)]
pub(crate) mod contract {
    use super::*;
    use crate::connectivity::core::{
        context::{ImsRoute, SipTransport},
        registration::{ImsRegistrationAccess, RegisteredImsContext},
    };
    use std::{
        collections::VecDeque,
        net::{Ipv4Addr, SocketAddr},
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum AuthenticatedExchangeStyle {
        SharedDriver,
        AdapterOwned,
    }

    struct ContractChannel {
        responses: VecDeque<Vec<u8>>,
        sends: Vec<Vec<u8>>,
    }

    impl ImsChannel for ContractChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sends.push(frame.to_vec());
            Ok(())
        }

        async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            self.responses
                .pop_front()
                .ok_or(ImsError::new("ims_register_contract_response_missing"))
        }

        fn route(&self) -> ImsRoute {
            ImsRoute {
                local_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 5060)),
                pcscf_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 5060)),
                transport: SipTransport::Udp,
            }
        }

        fn security_verify(&self) -> Option<&str> {
            None
        }
    }

    struct ContractAuthenticator {
        style: AuthenticatedExchangeStyle,
        challenges: Vec<(u16, u32, bool)>,
    }

    impl ContractAuthenticator {
        fn request_for(&mut self, challenge: &[u8], cseq: u32) -> Result<Vec<u8>, ImsError> {
            let status = sip_frame::parse_status(challenge)?;
            let resync = sip_frame::header_value(challenge, "X-Test-Auts")
                .is_some_and(|value| value.eq_ignore_ascii_case("required"));
            self.challenges.push((status, cseq, resync));
            let header = if status == 407 {
                "Proxy-Authorization"
            } else {
                "Authorization"
            };
            let proof = if resync {
                "Digest auts=contract"
            } else {
                "Digest response=contract"
            };
            Ok(format!(
                "REGISTER sip:ims.example SIP/2.0\r\nCSeq: {cseq} REGISTER\r\n{header}: {proof}\r\nContent-Length: 0\r\n\r\n"
            )
            .into_bytes())
        }
    }

    impl RegisterAuthenticator<ContractChannel> for ContractAuthenticator {
        async fn exchange_authenticated(
            &mut self,
            challenge_response: &[u8],
            cseq: u32,
            channel: &mut ContractChannel,
        ) -> Result<Option<Vec<u8>>, ImsError> {
            if self.style == AuthenticatedExchangeStyle::SharedDriver {
                return Ok(None);
            }
            let request = self.request_for(challenge_response, cseq)?;
            channel.send_sip(&request).await?;
            for _ in 0..MAX_REGISTER_PROVISIONAL_RESPONSES {
                let response = channel.recv_sip(REGISTER_TIMEOUT).await?;
                let status = sip_frame::parse_status(&response)?;
                if !(100..=199).contains(&status) {
                    return Ok(Some(response));
                }
            }
            Err(ImsError::new(
                "ims_register_authenticated_unexpected_status",
            ))
        }

        async fn authenticated_request(
            &mut self,
            challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            self.request_for(challenge_response, cseq)
        }
    }

    fn response(status: u16, reason: &str, headers: &str) -> Vec<u8> {
        format!("SIP/2.0 {status} {reason}\r\n{headers}Content-Length: 0\r\n\r\n").into_bytes()
    }

    fn success() -> Vec<u8> {
        response(
            200,
            "OK",
            concat!(
                "Contact: <sip:user@192.0.2.2>;expires=120\r\n",
                "Expires: 3600\r\n",
                "Service-Route: <sip:route.ims.example;lr>\r\n",
                "P-Associated-URI: <sip:+601100000001@ims.example>, <tel:+601100000001>\r\n",
            ),
        )
    }

    async fn exchange(
        style: AuthenticatedExchangeStyle,
        responses: impl IntoIterator<Item = Vec<u8>>,
    ) -> (
        Result<RegisterResult, RegisterFailure>,
        ContractChannel,
        ContractAuthenticator,
    ) {
        let mut channel = ContractChannel {
            responses: responses.into_iter().collect(),
            sends: Vec::new(),
        };
        let mut authenticator = ContractAuthenticator {
            style,
            challenges: Vec::new(),
        };
        let result = run_register_observed(
            &mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nCSeq: 1 REGISTER\r\nContent-Length: 0\r\n\r\n",
            &mut authenticator,
        )
        .await;
        (result, channel, authenticator)
    }

    fn assert_success_context(access: ImsRegistrationAccess, result: &RegisterResult) {
        let context = RegisteredImsContext::from_response(access, &result.response, 7200);
        assert_eq!(context.access, access);
        assert_eq!(context.lease.expires_seconds, 120);
        assert_eq!(
            context.service_route.as_deref(),
            Some("<sip:route.ims.example;lr>")
        );
        assert_eq!(
            context.associated_uris,
            ["sip:+601100000001@ims.example", "tel:+601100000001"]
        );
    }

    pub(crate) async fn assert_register_contract(
        style: AuthenticatedExchangeStyle,
        access: ImsRegistrationAccess,
    ) {
        let (direct, channel, authenticator) = exchange(style, [success()]).await;
        let direct = direct.expect("direct 200 REGISTER");
        assert!(!direct.authenticated);
        assert_eq!(direct.auth_rounds, 0);
        assert_eq!(channel.sends.len(), 1);
        assert!(authenticator.challenges.is_empty());
        assert_success_context(access, &direct);

        for challenge_status in [401, 407] {
            let (challenged, channel, authenticator) = exchange(
                style,
                [
                    response(100, "Trying", ""),
                    response(challenge_status, "Challenge", ""),
                    response(183, "Session Progress", ""),
                    success(),
                ],
            )
            .await;
            let challenged = challenged.expect("challenged REGISTER");
            assert!(challenged.authenticated);
            assert_eq!(challenged.auth_rounds, 1);
            assert_eq!(channel.sends.len(), 2);
            assert_eq!(authenticator.challenges, [(challenge_status, 2, false)]);
            let authenticated = String::from_utf8_lossy(&channel.sends[1]);
            assert!(authenticated.contains("CSeq: 2 REGISTER"));
            if challenge_status == 407 {
                assert!(authenticated.contains("Proxy-Authorization:"));
            } else {
                assert!(authenticated.contains("Authorization:"));
            }
            assert_success_context(access, &challenged);
        }

        let (resynchronized, channel, authenticator) = exchange(
            style,
            [
                response(401, "Unauthorized", "X-Test-Auts: required\r\n"),
                response(401, "Unauthorized", ""),
                success(),
            ],
        )
        .await;
        let resynchronized = resynchronized.expect("AUTS REGISTER");
        assert_eq!(resynchronized.auth_rounds, 2);
        assert_eq!(authenticator.challenges, [(401, 2, true), (401, 3, false)]);
        assert!(String::from_utf8_lossy(&channel.sends[1]).contains("auts=contract"));
        assert!(!String::from_utf8_lossy(&channel.sends[2]).contains("auts=contract"));
        assert_success_context(access, &resynchronized);

        let (rejected, channel, authenticator) = exchange(
            style,
            [
                response(401, "Unauthorized", ""),
                response(407, "Proxy Authentication Required", ""),
                response(401, "Unauthorized", ""),
            ],
        )
        .await;
        let rejected = rejected.expect_err("third challenge must be rejected");
        assert_eq!(rejected.error.code(), "ims_register_auth_rejected");
        assert_eq!(rejected.auth_rounds, 2);
        assert_eq!(channel.sends.len(), 3);
        assert_eq!(authenticator.challenges, [(401, 2, false), (407, 3, false)]);
    }
}
