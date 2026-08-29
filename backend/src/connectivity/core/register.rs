//! Transport-independent IMS REGISTER exchange.
//!
//! The channel owns TCP/UDP/IPsec/ESP details. The authenticator owns USIM AKA
//! and construction of the next authenticated REGISTER. This driver only
//! enforces the SIP transaction sequence and bounded challenge rounds.

use std::time::Duration;

use super::{access::ImsChannel, registration::UnregisterResult, sip_frame, ImsError};

const REGISTER_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_AUTH_ROUNDS: u8 = 2;
/// Safety valve for unrelated frames that share the IMS signaling path.
pub(crate) const MAX_REGISTER_IGNORED_FRAMES: u8 = 32;
/// Bounded `423 Interval Too Brief` negotiations per REGISTER exchange.
///
/// RFC 3261 §21.4.4 lets the registrar refuse a short lease with 423 plus a
/// `Min-Expires` value; the UE must retry with `Expires >= Min-Expires`.
/// Two rounds cover a core that raises the floor twice (for example after a
/// profile change) without letting a hostile registrar spin the loop forever.
pub(crate) const MAX_MIN_EXPIRES_ROUNDS: u8 = 2;
/// Upper bound applied to any advertised `Min-Expires`. Absurd floors (days,
/// years, or a malformed huge integer) are clamped so a single response cannot
/// pin the session lease to an unusable value.
pub(crate) const MIN_EXPIRES_CAP: u32 = 86_400;
pub(crate) const MAX_REGISTER_PROVISIONAL_RESPONSES: u8 = 4;

/// Identity of one SIP REGISTER client transaction.
///
/// A signaling connection is shared by REGISTER, NOTIFY, MESSAGE and dialog
/// traffic. Call-ID alone identifies a registration/dialog family, not one
/// request inside it; CSeq distinguishes the initial REGISTER from an
/// authenticated retry, a 423 retry and a later refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegisterTransactionKey {
    call_id: String,
    cseq: u32,
}

impl RegisterTransactionKey {
    pub(crate) fn from_register_request(request: &[u8]) -> Option<Self> {
        if !sip_frame::is_request(request, "REGISTER") {
            return None;
        }
        Some(Self {
            call_id: register_call_id(request)?,
            cseq: register_cseq(request)?,
        })
    }

    pub(crate) fn matches_response(&self, response: &[u8]) -> bool {
        response.starts_with(b"SIP/2.0")
            && register_call_id(response).as_deref() == Some(self.call_id.as_str())
            && register_cseq(response) == Some(self.cseq)
    }
}

fn register_call_id(frame: &[u8]) -> Option<String> {
    // RFC 3261 defines `i` as the compact Call-ID form. IMS peers normally
    // emit the long name, but accepting both avoids rejecting a legal response.
    sip_frame::header_value(frame, "Call-ID").or_else(|| sip_frame::header_value(frame, "i"))
}

fn register_cseq(frame: &[u8]) -> Option<u32> {
    let value = sip_frame::header_value(frame, "CSeq")?;
    let mut fields = value.split_whitespace();
    let number = fields.next()?.parse::<u32>().ok()?;
    let method = fields.next()?;
    method.eq_ignore_ascii_case("REGISTER").then_some(number)
}

/// Final-response statuses that a differently shaped REGISTER candidate (or a
/// different P-CSCF) may clear. Format, lease, extension, routing and
/// transient server failures are all candidates; anything that names a
/// policy/identity problem is not. The candidate ladder is bounded by every
/// adapter, so a broad retryable set costs at most a few extra attempts.
pub(crate) fn status_permits_register_variant_fallback(status: u16) -> bool {
    matches!(
        status,
        400 | 404
            | 408
            | 410
            | 415
            | 420
            | 421
            | 423
            | 430
            | 480
            | 491
            | 494
            | 500
            | 501
            | 502
            | 503
            | 504
    )
}

/// Final-response statuses for which retrying another REGISTER shape is
/// pointless: the server stated a policy/identity/format problem that header
/// experimentation cannot address, or a redirect this project does not follow.
/// Adapters should give up immediately instead of exhausting the ladder.
pub(crate) fn status_is_terminal_register_failure(status: u16) -> bool {
    matches!(
        status,
        300..=399
            | 403 | 405 | 406 | 409 | 413 | 414 | 416 | 422 | 432 | 433 | 436 | 437 | 438
            | 481 | 482 | 483 | 484 | 485 | 486 | 487 | 488 | 489 | 493 | 505 | 513 | 580
            | 600..=699
    )
}

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

    /// Rebuild REGISTER after a `423 Interval Too Brief` rejection.
    ///
    /// The shared driver owns the retry loop and enforces the round bound; an
    /// authenticator only has to produce the next request honoring
    /// `min_expires`. The default rebuilds an authenticated request through
    /// `authenticated_request`, which is correct for legs whose authenticator
    /// already owns every request shape. Legs that build the unauthenticated
    /// request themselves (VoLTE/VoWiFi) override the `authenticated == false`
    /// arm so the initial shape is reconstructed with the new lease.
    async fn rebuild_register_with_min_expires(
        &mut self,
        challenge_response: &[u8],
        cseq: u32,
        _min_expires: u32,
        authenticated: bool,
    ) -> Result<Vec<u8>, ImsError> {
        if authenticated {
            self.authenticated_request(challenge_response, cseq).await
        } else {
            Err(ImsError::new(
                "ims_register_initial_min_expires_unsupported",
            ))
        }
    }
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
    let expected_transaction = RegisterTransactionKey::from_register_request(initial_request);
    let mut response = recv_final_register_response(
        channel,
        expected_transaction.as_ref(),
        "ims_register_initial_receive_failed",
        "ims_register_initial_unexpected_status",
    )
    .await
    .map_err(|error| RegisterFailure::new(error, None, 0))?;
    let mut auth_rounds = 0u8;
    let mut min_expires_rounds = 0u8;

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
                let cseq = u32::from(auth_rounds) + u32::from(min_expires_rounds) + 1;
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
                    let expected_transaction =
                        RegisterTransactionKey::from_register_request(&request);
                    response = recv_final_register_response(
                        channel,
                        expected_transaction.as_ref(),
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
            423 if min_expires_rounds < MAX_MIN_EXPIRES_ROUNDS => {
                // RFC 3261 21.4.4: the registrar requires a longer lease.
                // Rebuild the current request shape (initial or authenticated)
                // with Expires >= Min-Expires and keep the same binding.
                let Some(min_expires) = parse_min_expires(&response) else {
                    let error = if auth_rounds == 0 {
                        "ims_register_initial_min_expires_invalid"
                    } else {
                        "ims_register_authenticated_min_expires_invalid"
                    };
                    tracing::warn!(
                        sip_status = status,
                        auth_rounds,
                        "IMS REGISTER 423 without a usable Min-Expires"
                    );
                    return Err(RegisterFailure::new(
                        ImsError::new(error),
                        Some(response),
                        auth_rounds,
                    ));
                };
                // The caller's initial REGISTER already consumed CSeq 1, so a
                // 423 retry starts at 2; each later round (auth or Min-Expires)
                // advances the sequence by one.
                let cseq = u32::from(auth_rounds) + u32::from(min_expires_rounds) + 2;
                min_expires_rounds += 1;
                let request = authenticator
                    .rebuild_register_with_min_expires(
                        &response,
                        cseq,
                        min_expires,
                        auth_rounds > 0,
                    )
                    .await
                    .map_err(|error| {
                        RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                    })?;
                let (send_error, receive_error, unexpected_error) = if auth_rounds == 0 {
                    (
                        "ims_register_initial_send_failed",
                        "ims_register_initial_receive_failed",
                        "ims_register_initial_unexpected_status",
                    )
                } else {
                    (
                        "ims_register_authenticated_send_failed",
                        "ims_register_authenticated_receive_failed",
                        "ims_register_authenticated_unexpected_status",
                    )
                };
                channel.send_sip(&request).await.map_err(|_| {
                    RegisterFailure::new(
                        ImsError::new(send_error),
                        Some(response.clone()),
                        auth_rounds,
                    )
                })?;
                let expected_transaction = RegisterTransactionKey::from_register_request(&request);
                response = recv_final_register_response(
                    channel,
                    expected_transaction.as_ref(),
                    receive_error,
                    unexpected_error,
                )
                .await
                .map_err(|error| RegisterFailure::new(error, None, auth_rounds))?;
            }
            423 => {
                let error = if auth_rounds == 0 {
                    "ims_register_initial_min_expires_exhausted"
                } else {
                    "ims_register_authenticated_min_expires_exhausted"
                };
                tracing::warn!(
                    sip_status = status,
                    auth_rounds,
                    min_expires_rounds,
                    "IMS REGISTER Min-Expires negotiation exhausted"
                );
                return Err(RegisterFailure::new(
                    ImsError::new(error),
                    Some(response),
                    auth_rounds,
                ));
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
    expected_transaction: Option<&RegisterTransactionKey>,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    recv_final_register_response_with_timeout(
        channel,
        expected_transaction,
        receive_error,
        provisional_exhausted_error,
        REGISTER_TIMEOUT,
    )
    .await
}

async fn recv_final_register_response_with_timeout<C>(
    channel: &mut C,
    expected_transaction: Option<&RegisterTransactionKey>,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
    timeout_budget: Duration,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    // One absolute budget for the whole transaction. Unrelated traffic and
    // provisional responses must not restart the timer for each frame.
    let deadline = tokio::time::Instant::now() + timeout_budget;
    let mut provisional_count = 0u8;
    let mut ignored_frames = 0u8;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ImsError::new(receive_error));
        }
        let response = channel
            .recv_sip_fresh(remaining)
            .await
            .map_err(|_| ImsError::new(receive_error))?;
        // The IMS signaling path is shared with in-dialog requests (NOTIFY,
        // MESSAGE, ...). Only a response for this REGISTER transaction may be
        // consumed; anything else goes back to the caller's queue so the
        // session loop can handle it instead of killing the exchange.
        let is_response = response.starts_with(b"SIP/2.0");
        let transaction_matches = expected_transaction
            .map(|expected| expected.matches_response(&response))
            .unwrap_or(is_response);
        if !transaction_matches {
            ignored_frames = ignored_frames.saturating_add(1);
            channel.requeue(response);
            if ignored_frames > MAX_REGISTER_IGNORED_FRAMES {
                return Err(ImsError::new(receive_error));
            }
            tracing::debug!(
                is_response,
                transaction_key_available = expected_transaction.is_some(),
                "IMS REGISTER skipping frame outside the current transaction"
            );
            continue;
        }
        let status = sip_frame::parse_status(&response)?;
        if !(100..=199).contains(&status) {
            return Ok(response);
        }
        provisional_count = provisional_count.saturating_add(1);
        if provisional_count > MAX_REGISTER_PROVISIONAL_RESPONSES {
            break;
        }
        tracing::debug!(
            sip_status = status,
            provisional_count,
            "IMS REGISTER provisional response received"
        );
    }
    Err(ImsError::new(provisional_exhausted_error))
}

/// Parse the `Min-Expires` header from a `423 Interval Too Brief` response.
///
/// RFC 3261 21.4.4 requires the registrar to include the minimum lease it will
/// accept. Missing, malformed, or absurd values are clamped defensively so the
/// retry always carries a positive, bounded lease.
fn parse_min_expires(response: &[u8]) -> Option<u32> {
    let value = sip_frame::header_value(response, "Min-Expires")?;
    let parsed = value.trim().parse::<u32>().ok()?;
    Some(parsed.min(MIN_EXPIRES_CAP).max(1))
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
        requeued: VecDeque<Vec<u8>>,
        sends: Vec<Vec<u8>>,
        transport: SipTransport,
    }

    impl ImsChannel for FakeChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sends.push(frame.to_vec());
            Ok(())
        }
        async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            if let Some(frame) = self.requeued.pop_front() {
                return Ok(frame);
            }
            self.responses
                .pop_front()
                .ok_or(ImsError::new("ims_test_response_missing"))
        }
        async fn recv_sip_fresh(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            self.responses
                .pop_front()
                .ok_or(ImsError::new("ims_test_response_missing"))
        }
        fn requeue(&mut self, frame: Vec<u8>) {
            self.requeued.push_back(frame);
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

    struct TimedUnrelatedChannel {
        requeued: VecDeque<Vec<u8>>,
        completed_reads: usize,
        frame_delay: Duration,
    }

    impl ImsChannel for TimedUnrelatedChannel {
        async fn send_sip(&mut self, _frame: &[u8]) -> Result<(), ImsError> {
            Ok(())
        }

        async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
            if let Some(frame) = self.requeued.pop_front() {
                return Ok(frame);
            }
            self.recv_sip_fresh(timeout).await
        }

        async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
            tokio::time::timeout(timeout, tokio::time::sleep(self.frame_delay))
                .await
                .map_err(|_| ImsError::new("ims_channel_read_timeout"))?;
            self.completed_reads += 1;
            Ok(b"NOTIFY sip:ua@ims.example SIP/2.0\r\nCall-ID: other@dev\r\nContent-Length: 0\r\n\r\n".to_vec())
        }

        fn requeue(&mut self, frame: Vec<u8>) {
            self.requeued.push_back(frame);
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

    struct MinExpiresAuthenticator {
        rebuilds: Vec<(u32, bool)>,
    }

    impl RegisterAuthenticator<FakeChannel> for MinExpiresAuthenticator {
        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(format!("REGISTER auth CSeq {cseq}").into_bytes())
        }

        async fn rebuild_register_with_min_expires(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
            min_expires: u32,
            authenticated: bool,
        ) -> Result<Vec<u8>, ImsError> {
            self.rebuilds.push((min_expires, authenticated));
            Ok(format!("REGISTER {cseq} Expires {min_expires}").into_bytes())
        }
    }

    fn response(code: u16, reason: &str) -> Vec<u8> {
        format!("SIP/2.0 {code} {reason}\r\nContent-Length: 0\r\n\r\n").into_bytes()
    }

    fn response_with_header(code: u16, reason: &str, headers: &str) -> Vec<u8> {
        format!("SIP/2.0 {code} {reason}\r\n{headers}Content-Length: 0\r\n\r\n").into_bytes()
    }

    #[tokio::test]
    async fn initial_challenge_authenticated_success() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response(401, "Unauthorized"), response(200, "OK")]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        assert_eq!(
            run_unregister(&mut rejected, b"unregister", &mut FakeAuthenticator).await,
            UnregisterResult::Rejected
        );

        let mut lost = FakeChannel {
            responses: VecDeque::new(),
            sends: Vec::new(),
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
    async fn provisional_response_limit_allows_exactly_the_documented_count() {
        let mut responses = VecDeque::new();
        for _ in 0..MAX_REGISTER_PROVISIONAL_RESPONSES {
            responses.push_back(response(100, "Trying"));
        }
        responses.push_back(response(200, "OK"));
        let mut channel = FakeChannel {
            responses,
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };

        run_register(&mut channel, b"REGISTER initial", &mut FakeAuthenticator)
            .await
            .unwrap();
        assert!(channel.responses.is_empty());
    }

    #[tokio::test]
    async fn provisional_response_loop_is_bounded() {
        let mut responses = VecDeque::new();
        for _ in 0..=MAX_REGISTER_PROVISIONAL_RESPONSES {
            responses.push_back(response(100, "Trying"));
        }
        responses.push_back(response(401, "Unauthorized"));
        let mut channel = FakeChannel {
            responses,
            sends: Vec::new(),
            requeued: VecDeque::new(),
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
    async fn ignored_and_provisional_limits_are_counted_independently() {
        let mut responses = VecDeque::new();
        for _ in 0..MAX_REGISTER_IGNORED_FRAMES {
            responses.push_back(
                b"OPTIONS sip:ua@ims.example SIP/2.0\r\nCall-ID: other@dev\r\nContent-Length: 0\r\n\r\n"
                    .to_vec(),
            );
        }
        for _ in 0..MAX_REGISTER_PROVISIONAL_RESPONSES {
            responses.push_back(response(100, "Trying"));
        }
        responses.push_back(response(200, "OK"));
        let mut channel = FakeChannel {
            responses,
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };

        run_register(&mut channel, b"REGISTER initial", &mut FakeAuthenticator)
            .await
            .unwrap();

        assert_eq!(channel.requeued.len(), MAX_REGISTER_IGNORED_FRAMES as usize);
        assert!(channel.responses.is_empty());
    }

    #[tokio::test]
    async fn unrelated_frames_do_not_restart_the_absolute_register_deadline() {
        let timeout_budget = Duration::from_millis(40);
        let mut channel = TimedUnrelatedChannel {
            requeued: VecDeque::new(),
            completed_reads: 0,
            frame_delay: Duration::from_millis(15),
        };
        let started = tokio::time::Instant::now();

        let error = recv_final_register_response_with_timeout(
            &mut channel,
            None,
            "ims_register_initial_receive_failed",
            "ims_register_initial_unexpected_status",
            timeout_budget,
        )
        .await
        .unwrap_err();
        let elapsed = tokio::time::Instant::now() - started;

        assert_eq!(error.code(), "ims_register_initial_receive_failed");
        assert!(elapsed >= timeout_budget, "elapsed only {elapsed:?}");
        assert!(elapsed < Duration::from_millis(150), "elapsed {elapsed:?}");
        assert_eq!(channel.completed_reads, 2);
        assert_eq!(channel.requeued.len(), 2);
    }

    #[tokio::test]
    async fn unrelated_frames_on_the_shared_path_are_requeued_not_consumed() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                b"NOTIFY sip:ua@ims.example SIP/2.0\r\nCall-ID: other@dev\r\nContent-Length: 0\r\n\r\n".to_vec(),
                response_with_header(
                    200,
                    "OK",
                    "Call-ID: abc@dev\r\nCSeq: 1 REGISTER\r\n",
                ),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let result = run_register(
            &mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc@dev\r\nCSeq: 1 REGISTER\r\nContent-Length: 0\r\n\r\n",
            &mut auth,
        )
        .await
        .unwrap();

        assert!(!result.authenticated);
        assert!(channel.responses.is_empty());
        assert_eq!(channel.requeued.len(), 1);
        // The unrelated request is preserved for the session loop.
        let frame = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert!(frame.starts_with(b"NOTIFY"));
    }

    #[tokio::test]
    async fn mismatched_call_id_response_is_requeued_and_matching_one_wins() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(200, "OK", "Call-ID: stale@dev\r\nCSeq: 1 REGISTER\r\n"),
                response_with_header(200, "OK", "Call-ID: abc@dev\r\nCSeq: 1 REGISTER\r\n"),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        run_register(
            &mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc@dev\r\nCSeq: 1 REGISTER\r\nContent-Length: 0\r\n\r\n",
            &mut auth,
        )
        .await
        .unwrap();

        assert!(channel.responses.is_empty());
        assert_eq!(channel.requeued.len(), 1);
        let frame = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert!(String::from_utf8_lossy(&frame).contains("stale@dev"));
    }

    #[tokio::test]
    async fn only_matching_register_cseq_is_consumed() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(200, "OK", "CSeq: 8 REGISTER\r\n"),
                response_with_header(200, "OK", "Call-ID: abc@dev\r\nCSeq: 7 INVITE\r\n"),
                response_with_header(200, "OK", "Call-ID: abc@dev\r\nCSeq: 8 REGISTER\r\n"),
                response_with_header(200, "OK", "Call-ID: abc@dev\r\nCSeq: 7 REGISTER\r\n"),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };

        run_register(
            &mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc@dev\r\nCSeq: 7 REGISTER\r\nContent-Length: 0\r\n\r\n",
            &mut FakeAuthenticator,
        )
        .await
        .unwrap();

        assert!(channel.responses.is_empty());
        assert_eq!(channel.requeued.len(), 3);
    }

    #[test]
    fn register_transaction_key_requires_call_id_and_register_cseq() {
        assert!(RegisterTransactionKey::from_register_request(
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc\r\nCSeq: 1 REGISTER\r\n\r\n"
        )
        .is_some());
        assert!(RegisterTransactionKey::from_register_request(
            b"REGISTER sip:ims.example SIP/2.0\r\nCSeq: 1 REGISTER\r\n\r\n"
        )
        .is_none());
        assert!(RegisterTransactionKey::from_register_request(
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc\r\nCSeq: 1 INVITE\r\n\r\n"
        )
        .is_none());
    }

    #[tokio::test]
    async fn unrelated_frame_limit_allows_exactly_the_documented_count() {
        let mut responses: VecDeque<Vec<u8>> = (0..MAX_REGISTER_IGNORED_FRAMES)
            .map(|_| {
                b"NOTIFY sip:ua@ims.example SIP/2.0\r\nCall-ID: other@dev\r\nContent-Length: 0\r\n\r\n"
                    .to_vec()
            })
            .collect();
        responses.push_back(response(200, "OK"));
        let mut channel = FakeChannel {
            responses,
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };

        run_register(&mut channel, b"REGISTER initial", &mut FakeAuthenticator)
            .await
            .unwrap();
        assert_eq!(channel.requeued.len(), MAX_REGISTER_IGNORED_FRAMES as usize);
        assert!(channel.responses.is_empty());
    }

    #[tokio::test]
    async fn unrelated_frame_flood_bounds_the_register_wait() {
        let mut channel = FakeChannel {
            responses: (0..64)
                .map(|_| {
                    b"NOTIFY sip:ua@ims.example SIP/2.0\r\nCall-ID: other@dev\r\nContent-Length: 0\r\n\r\n"
                        .to_vec()
                })
                .collect(),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_initial_receive_failed");
        assert_eq!(
            channel.requeued.len(),
            MAX_REGISTER_IGNORED_FRAMES as usize + 1
        );
        assert_eq!(channel.responses.len(), 31);
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
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
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = FakeAuthenticator;

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_authenticated_receive_failed");
        assert_eq!(channel.sends.len(), 2);
    }

    #[tokio::test]
    async fn min_expires_negotiation_rebuilds_initial_register() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(423, "Interval Too Brief", "Min-Expires: 1800\r\n"),
                response(200, "OK"),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = MinExpiresAuthenticator {
            rebuilds: Vec::new(),
        };

        let result = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap();

        assert!(!result.authenticated);
        assert_eq!(result.auth_rounds, 0);
        assert_eq!(channel.sends.len(), 2);
        assert!(String::from_utf8_lossy(&channel.sends[1]).contains("Expires 1800"));
        assert_eq!(auth.rebuilds, [(1800, false)]);
    }

    #[tokio::test]
    async fn min_expires_negotiation_is_bounded() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 1200\r\n"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 3600\r\n"),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = MinExpiresAuthenticator {
            rebuilds: Vec::new(),
        };

        let error = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap_err();

        assert_eq!(error.code(), "ims_register_initial_min_expires_exhausted");
        assert_eq!(channel.sends.len(), 3);
        assert_eq!(auth.rebuilds, [(600, false), (1200, false)]);
    }

    #[tokio::test]
    async fn min_expires_after_challenge_keeps_authenticated_binding() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response(401, "Unauthorized"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 900\r\n"),
                response(200, "OK"),
            ]),
            sends: Vec::new(),
            requeued: VecDeque::new(),
            transport: SipTransport::Udp,
        };
        let mut auth = MinExpiresAuthenticator {
            rebuilds: Vec::new(),
        };

        let result = run_register(&mut channel, b"REGISTER initial", &mut auth)
            .await
            .unwrap();

        assert!(result.authenticated);
        assert_eq!(result.auth_rounds, 1);
        assert_eq!(channel.sends.len(), 3);
        assert!(String::from_utf8_lossy(&channel.sends[1]).contains("CSeq 2"));
        assert!(String::from_utf8_lossy(&channel.sends[2]).contains("Expires 900"));
        assert_eq!(auth.rebuilds, [(900, true)]);
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

        async fn rebuild_register_with_min_expires(
            &mut self,
            challenge_response: &[u8],
            cseq: u32,
            min_expires: u32,
            authenticated: bool,
        ) -> Result<Vec<u8>, ImsError> {
            if authenticated {
                self.request_for(challenge_response, cseq)
            } else {
                Ok(format!(
                    "REGISTER sip:ims.example SIP/2.0\r\nCSeq: {cseq} REGISTER\r\nExpires: {min_expires}\r\nContent-Length: 0\r\n\r\n"
                )
                .into_bytes())
            }
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

        let (negotiated, channel, _) = exchange(
            style,
            [
                response(423, "Interval Too Brief", "Min-Expires: 1800\r\n"),
                success(),
            ],
        )
        .await;
        let negotiated = negotiated.expect("423 with Min-Expires must retry");
        assert!(!negotiated.authenticated);
        assert_eq!(negotiated.auth_rounds, 0);
        assert_eq!(channel.sends.len(), 2);
        let retried = String::from_utf8_lossy(&channel.sends[1]);
        assert!(retried.contains("CSeq: 2 REGISTER"));
        assert!(retried.contains("Expires: 1800"));
        assert_success_context(access, &negotiated);

        let (exhausted, channel, _) = exchange(
            style,
            [
                response(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                response(423, "Interval Too Brief", "Min-Expires: 1200\r\n"),
                response(423, "Interval Too Brief", "Min-Expires: 3600\r\n"),
            ],
        )
        .await;
        let exhausted = exhausted.expect_err("repeated 423 must be bounded");
        assert_eq!(
            exhausted.error.code(),
            "ims_register_initial_min_expires_exhausted"
        );
        assert_eq!(channel.sends.len(), 3);
    }
}
