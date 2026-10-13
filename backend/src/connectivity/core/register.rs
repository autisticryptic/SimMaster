//! Transport-independent IMS REGISTER exchange.
//!
//! The channel owns TCP/UDP/IPsec/ESP details. The authenticator owns USIM AKA
//! and construction of the next authenticated REGISTER. This driver only
//! enforces the SIP transaction sequence and bounded challenge rounds.

use std::time::Duration;

use super::{
    access::ImsChannel, context::SipTransport, registration::UnregisterResult, sip_frame, ImsError,
};
pub use super::ims_failure::{RetryAfter, RetryAfterInvalid};

/// RFC 3261 T1. IMS REGISTER is a non-INVITE transaction and must retransmit
/// the identical request on an unreliable UDP transport when the response is
/// lost.
const REGISTER_T1: Duration = Duration::from_millis(500);
/// RFC 3261 T2 bounds the non-INVITE retransmission interval.
const REGISTER_T2: Duration = Duration::from_secs(4);
/// RFC 3261 Timer F is 64*T1 for a UDP client transaction.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(32);

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
/// Upper supported `Min-Expires` floor. Values above it are unsupported, not
/// clamped: a retry below the server's floor cannot satisfy RFC 3261.
pub(crate) const MIN_EXPIRES_CAP: u32 = 86_400;

/// Safe lease-negotiation provenance, independent of the final SIP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterMinExpiresFailure {
    Invalid,
    Unsupported,
    Exhausted,
    /// A valid floor was negotiated before a later status/transport/auth error.
    Negotiated,
}

impl RegisterMinExpiresFailure {
    pub fn error(self, authenticated: bool) -> ImsError {
        let code = match (authenticated, self) {
            (false, Self::Invalid) => "ims_register_initial_min_expires_invalid",
            (false, Self::Unsupported) => "ims_register_initial_min_expires_unsupported",
            (false, Self::Exhausted | Self::Negotiated) => "ims_register_initial_min_expires_exhausted",
            (true, Self::Invalid) => "ims_register_authenticated_min_expires_invalid",
            (true, Self::Unsupported) => "ims_register_authenticated_min_expires_unsupported",
            (true, Self::Exhausted | Self::Negotiated) => "ims_register_authenticated_min_expires_exhausted",
        };
        ImsError::new(code)
    }
}

/// Attempt-scoped, never global. Once a registrar negotiates a lease, callers
/// must not restart the header/profile/ESP candidate ladder, even if the next
/// response is 400 or the next send loses transport. Retries within the current
/// exchange retain the channel, request shape and authentication budget.
#[derive(Debug, Default)]
pub struct RegisterNegotiationBudget {
    observed: bool,
    rounds: u8,
    minimum: Option<u32>,
}

impl RegisterNegotiationBudget {
    pub fn permits_candidate_expansion(&self) -> bool {
        !self.observed
    }

    pub fn failure_metadata(&self, failure: &RegisterFailure) -> RegisterFailureMetadata {
        let mut metadata = failure.metadata();
        if self.observed && metadata.min_expires.is_none() {
            metadata.min_expires = Some(RegisterMinExpiresFailure::Negotiated);
        }
        metadata
    }

    fn negotiate(&mut self, response: &[u8]) -> Result<u32, RegisterMinExpiresFailure> {
        self.observed = true;
        if self.rounds >= MAX_MIN_EXPIRES_ROUNDS {
            return Err(RegisterMinExpiresFailure::Exhausted);
        }
        let minimum = parse_min_expires(response)?;
        if self.minimum.is_some_and(|previous| minimum <= previous) {
            return Err(RegisterMinExpiresFailure::Invalid);
        }
        self.minimum = Some(minimum);
        self.rounds += 1;
        Ok(minimum)
    }
}
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
    (method.eq_ignore_ascii_case("REGISTER") && fields.next().is_none()).then_some(number)
}

/// Final-response statuses that can plausibly be fixed by a different
/// REGISTER header shape. Do not classify a timeout, a server failure, a lease
/// negotiation failure, or a policy/identity response as header compatibility:
/// changing PANI/Route/security formatting after those responses only burns the
/// existing registration's recovery window. `423` is handled in the shared
/// driver on the same channel and must never enter this candidate ladder.
pub(crate) fn status_permits_register_variant_fallback(status: u16) -> bool {
    matches!(status, 400 | 415 | 420 | 421 | 494)
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
    /// Bind retained proof context to the final serialized request before any
    /// send. Access-owned exchanges must call this (or their equivalent) before
    /// sending too; a builder's intended URI is not the actual wire request.
    fn validate_request(&self, _request: &[u8]) -> Result<(), ImsError> { Ok(()) }

    /// Validate a final success before the driver exposes it as a registered
    /// binding. AUTS-only responses and response-authentication failures must
    /// not commit a lease/security state merely because the SIP status is 2xx.
    fn validate_success(&self, _response: &[u8]) -> Result<(), ImsError> { Ok(()) }

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
    /// The shared driver owns the retry loop and enforces the round bound.
    /// Adapters must explicitly rebuild the same request shape and, if needed,
    /// a fresh Digest proof. Passing a 423 to the challenge parser or replaying
    /// Authorization with the old nonce-count is not a valid implementation.
    async fn rebuild_register_with_min_expires(
        &mut self,
        _challenge_response: &[u8],
        _cseq: u32,
        _min_expires: u32,
        authenticated: bool,
    ) -> Result<Vec<u8>, ImsError> {
        Err(RegisterMinExpiresFailure::Unsupported.error(authenticated))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterResult {
    pub response: Vec<u8>,
    pub authenticated: bool,
    pub auth_rounds: u8,
}

/// Stage within the shared exchange, not the adapter's initial/refresh lifecycle.
/// In particular, a protected refresh's first request is still `Initial` here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterFailureStage {
    Initial,
    /// Challenge processing/AKA/security preparation failed before the next send.
    Authentication,
    Authenticated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterFailureKind {
    /// A final 408/500/502/503/504; not a REGISTER header compatibility hint.
    TemporaryEndpoint,
    /// No final result from this send. Any saved response is historical.
    Transport,
    /// The established outbound flow is positively known to be unusable.
    FlowFailed,
    IdentityRejected,
    AuthenticationRejected,
    /// Initial header/security compatibility or bounded 423 negotiation failure.
    Negotiation,
    TerminalResponse,
    /// Local parsing, AKA or security setup failure; never authorizes fallback.
    LocalFailure,
}

/// Safe to copy into access-adapter errors. Contains no identity, challenge,
/// carrier reason, authorization or security material. `sip_status` is the last
/// observed status, not necessarily the result of the last send: use `kind`
/// rather than interpreting a saved 401 as an authentication rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterFailureMetadata {
    pub sip_status: Option<u16>,
    pub stage: RegisterFailureStage,
    pub kind: RegisterFailureKind,
    pub retry_after: RetryAfter,
    /// Any 423 closes compatibility expansion, including a later 400/timeout.
    pub min_expires: Option<RegisterMinExpiresFailure>,
}

impl RegisterFailureMetadata {
    pub fn is_temporary(self) -> bool {
        matches!(self.kind, RegisterFailureKind::TemporaryEndpoint | RegisterFailureKind::Transport)
    }

    /// This is only an eligibility gate. Existing security, identity and
    /// candidate-budget checks must still run; it never authorizes bare UDP.
    pub fn permits_variant_fallback(self) -> bool {
        self.min_expires.is_none()
            && self.stage == RegisterFailureStage::Initial
            && self.kind == RegisterFailureKind::Negotiation
            && self.retry_after == RetryAfter::Absent
            && self.sip_status.is_some_and(status_permits_register_variant_fallback)
    }
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
    /// Derive structured metadata without changing the public literal shape of
    /// RegisterFailure. Stable error codes identify provenance; display/detail
    /// strings and carrier Warning text do not influence recovery decisions.
    pub fn metadata(&self) -> RegisterFailureMetadata {
        use RegisterFailureKind as Kind;
        use RegisterFailureStage as Stage;
        let code = self.error.code();
        let sip_status = self.response.as_deref()
            .and_then(|response| sip_frame::parse_status(response).ok())
            .filter(|status| (100..=699).contains(status));
        let retry_after = self.response.as_deref()
            .map(super::ims_failure::parse_retry_after)
            .unwrap_or(RetryAfter::Absent);
        let authenticated_exchange = code.starts_with("ims_register_authenticated_")
            || code == "ims_register_auth_rejected";
        let stage = if authenticated_exchange {
            Stage::Authenticated
        } else if self.auth_rounds > 0 {
            Stage::Authentication
        } else {
            Stage::Initial
        };
        let transport = matches!(code,
            "ims_register_initial_send_failed" | "ims_register_initial_receive_failed"
            | "ims_register_authenticated_send_failed" | "ims_register_authenticated_receive_failed");
        let final_response = matches!(code,
            "ims_register_initial_unexpected_status" | "ims_register_authenticated_unexpected_status"
            | "ims_register_auth_rejected" | "ims_register_initial_min_expires_invalid"
            | "ims_register_initial_min_expires_exhausted" | "ims_register_initial_min_expires_unsupported"
            | "ims_register_authenticated_min_expires_invalid" | "ims_register_authenticated_min_expires_exhausted"
            | "ims_register_authenticated_min_expires_unsupported");
        let kind = if code.starts_with("digest_response_auth_") {
            // A 2xx with invalid server authentication is not a successful
            // registration, transport loss or a header-compatibility hint.
            Kind::AuthenticationRejected
        } else if transport {
            Kind::Transport
        } else if matches!(code,
            "ims_outbound_flow_failed" | "ims_outbound_keepalive_timeout"
            | "ims_outbound_keepalive_send_failed" | "ims_outbound_stun_error"
            | "ims_outbound_mapping_changed")
        {
            Kind::FlowFailed
        } else if final_response {
            match sip_status {
                Some(408 | 500 | 502 | 503 | 504) => Kind::TemporaryEndpoint,
                Some(402 | 403 | 404 | 604) => Kind::IdentityRejected,
                Some(401 | 407) => Kind::AuthenticationRejected,
                Some(430) => Kind::FlowFailed,
                Some(423) => Kind::Negotiation,
                Some(status) if stage == Stage::Initial
                    && status_permits_register_variant_fallback(status) => Kind::Negotiation,
                Some(300..=699) => Kind::TerminalResponse,
                _ => Kind::LocalFailure,
            }
        } else {
            Kind::LocalFailure
        };
        let min_expires = if code.contains("_min_expires_") {
            if code.ends_with("_invalid") {
                Some(RegisterMinExpiresFailure::Invalid)
            } else if code.ends_with("_unsupported") {
                Some(RegisterMinExpiresFailure::Unsupported)
            } else if code.ends_with("_exhausted") {
                Some(RegisterMinExpiresFailure::Exhausted)
            } else {
                None
            }
        } else if sip_status == Some(423) {
            Some(RegisterMinExpiresFailure::Negotiated)
        } else {
            None
        };
        RegisterFailureMetadata { sip_status, stage, kind, retry_after, min_expires }
    }

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
    run_register_observed_with_budget(
        channel, initial_request, authenticator, &mut RegisterNegotiationBudget::default(),
    ).await
}

/// Candidate-aware wrapper. Keep this budget outside the compatibility loop,
/// and propagate `budget.failure_metadata(&failure)` on failure. Re-entering
/// with an already negotiated budget fails before another request is sent.
pub async fn run_register_observed_with_budget<C, A>(
    channel: &mut C,
    initial_request: &[u8],
    authenticator: &mut A,
    budget: &mut RegisterNegotiationBudget,
) -> Result<RegisterResult, RegisterFailure>
where
    C: ImsChannel,
    A: RegisterAuthenticator<C>,
{
    if !budget.permits_candidate_expansion() {
        return Err(RegisterFailure::new(
            RegisterMinExpiresFailure::Exhausted.error(false), None, 0,
        ));
    }
    // Guard even callers that accidentally route Expires: 0 through REGISTER.
    let unregister = sip_frame::header_values(initial_request, "Expires")
        .iter().any(|value| value.trim() == "0");
    run_register_exchange(channel, initial_request, authenticator, !unregister, budget).await
}

// Deregistration must never negotiate a positive lease in response to 423.
async fn run_register_exchange<C, A>(
    channel: &mut C,
    initial_request: &[u8],
    authenticator: &mut A,
    allow_min_expires: bool,
    budget: &mut RegisterNegotiationBudget,
) -> Result<RegisterResult, RegisterFailure>
where
    C: ImsChannel,
    A: RegisterAuthenticator<C>,
{
    budget.minimum = sip_frame::header_value(initial_request, "Expires")
        .and_then(|value| value.trim().parse::<u32>().ok());
    authenticator.validate_request(initial_request)
        .map_err(|error| RegisterFailure::new(error, None, 0))?;
    let mut response = send_register_and_receive(
        channel,
        initial_request,
        "ims_register_initial_send_failed",
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
                authenticator.validate_success(&response)
                    .map_err(|error| RegisterFailure::new(error, Some(response.clone()), auth_rounds))?;
                return Ok(RegisterResult {
                    response,
                    authenticated: auth_rounds > 0,
                    auth_rounds,
                })
            }
            401 | 407 if auth_rounds < MAX_AUTH_ROUNDS => {
                auth_rounds += 1;
                let cseq = u32::from(auth_rounds) + u32::from(budget.rounds) + 1;
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
                    authenticator.validate_request(&request)
                        .map_err(|error| RegisterFailure::new(error, Some(response.clone()), auth_rounds))?;
                    response = send_register_and_receive(
                        channel,
                        &request,
                        "ims_register_authenticated_send_failed",
                        "ims_register_authenticated_receive_failed",
                        "ims_register_authenticated_unexpected_status",
                    )
                    .await
                    .map_err(|error| {
                        RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                    })?;
                }
            }
            401 | 407 => {
                return Err(RegisterFailure::new(
                    ImsError::new("ims_register_auth_rejected"),
                    Some(response),
                    auth_rounds,
                ))
            }
            423 => {
                // Never let a rejected lease reopen compatibility discovery.
                budget.observed = true;
                let min_expires = if allow_min_expires {
                    budget.negotiate(&response)
                } else {
                    Err(RegisterMinExpiresFailure::Unsupported)
                }.map_err(|reason| RegisterFailure::new(
                    reason.error(auth_rounds > 0), Some(response.clone()), auth_rounds,
                ))?;
                let cseq = u32::from(auth_rounds) + u32::from(budget.rounds) + 1;
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
                authenticator.validate_request(&request)
                    .map_err(|error| RegisterFailure::new(error, Some(response.clone()), auth_rounds))?;
                response = send_register_and_receive(
                    channel,
                    &request,
                    send_error,
                    receive_error,
                    unexpected_error,
                )
                .await
                .map_err(|error| {
                    RegisterFailure::new(error, Some(response.clone()), auth_rounds)
                })?;
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
    match run_unregister_observed(channel, initial_request, authenticator).await {
        Ok(_) => UnregisterResult::Confirmed,
        Err(failure) if failure.response.is_some() => UnregisterResult::Rejected,
        Err(_) => UnregisterResult::AccessLost,
    }
}

/// Same removal exchange, retaining structured evidence for passive runtime
/// diagnostics. Callers must not serialize the response or authentication data.
pub async fn run_unregister_observed<C, A>(
    channel: &mut C, initial_request: &[u8], authenticator: &mut A,
) -> Result<RegisterResult, RegisterFailure>
where C: ImsChannel, A: RegisterAuthenticator<C> {
    run_register_exchange(channel, initial_request, authenticator, false, &mut RegisterNegotiationBudget::default()).await
}

fn preserve_flow_error(error: ImsError, fallback: &'static str) -> ImsError {
    if error.code().starts_with("ims_outbound_")
        || error.code().starts_with("cellular_ims_tls_")
        || matches!(error.code(), "cellular_ims_channel_stream_frame_invalid"
            | "cellular_ims_channel_stream_response_owner_missing"
            | "cellular_ims_channel_stream_budget_exhausted")
    {
        error
    } else {
        ImsError::new(fallback)
    }
}

async fn send_register_and_receive<C>(
    channel: &mut C,
    request: &[u8],
    send_error: &'static str,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    let expected_transaction = RegisterTransactionKey::from_register_request(request);
    channel
        .send_sip(request)
        .await
        .map_err(|error| preserve_flow_error(error, send_error))?;
    recv_final_register_response_with_retransmit(
        channel,
        request,
        expected_transaction.as_ref(),
        send_error,
        receive_error,
        provisional_exhausted_error,
    )
    .await
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
    recv_final_register_response_loop(
        channel,
        None,
        expected_transaction,
        None,
        receive_error,
        provisional_exhausted_error,
        REGISTER_TIMEOUT,
    )
    .await
}

async fn recv_final_register_response_with_retransmit<C>(
    channel: &mut C,
    request: &[u8],
    expected_transaction: Option<&RegisterTransactionKey>,
    send_error: &'static str,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    let retransmit = (channel.route().transport == SipTransport::Udp).then_some(request);
    recv_final_register_response_loop(
        channel,
        retransmit,
        expected_transaction,
        Some(send_error),
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
    recv_final_register_response_loop(
        channel,
        None,
        expected_transaction,
        None,
        receive_error,
        provisional_exhausted_error,
        timeout_budget,
    )
    .await
}

async fn recv_final_register_response_loop<C>(
    channel: &mut C,
    retransmit_request: Option<&[u8]>,
    expected_transaction: Option<&RegisterTransactionKey>,
    send_error: Option<&'static str>,
    receive_error: &'static str,
    provisional_exhausted_error: &'static str,
    timeout_budget: Duration,
) -> Result<Vec<u8>, ImsError>
where
    C: ImsChannel,
{
    // One absolute budget for the whole transaction. Unrelated traffic and
    // provisional responses must not restart the timer for each frame.
    // For UDP, the same REGISTER transaction is retransmitted according to
    // RFC 3261 Timer E/T2; retransmission keeps the same Call-ID, branch and
    // CSeq. A refresh timeout must not be turned into a brand-new REGISTER
    // transaction merely because one datagram was lost.
    let deadline = tokio::time::Instant::now() + timeout_budget;
    let udp_retransmit = retransmit_request.is_some();
    let mut retransmit_interval = REGISTER_T1;
    let mut next_retransmit = tokio::time::Instant::now() + retransmit_interval;
    let mut provisional_count = 0u8;
    let mut ignored_frames = 0u8;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(ImsError::new(receive_error));
        }
        if udp_retransmit && now >= next_retransmit {
            let request = retransmit_request.expect("UDP retransmission request is present");
            channel
                .send_sip(request)
                .await
                .map_err(|error| preserve_flow_error(error, send_error.unwrap_or(receive_error)))?;
            tracing::debug!(
                register_cseq = sip_frame::header_value(request, "CSeq"),
                interval_ms = retransmit_interval.as_millis(),
                "IMS REGISTER retransmitting UDP transaction"
            );
            retransmit_interval = retransmit_interval.saturating_mul(2).min(REGISTER_T2);
            next_retransmit = now + retransmit_interval;
            continue;
        }
        let deadline_for_read = if udp_retransmit {
            deadline.min(next_retransmit)
        } else {
            deadline
        };
        let remaining = deadline_for_read.saturating_duration_since(now);
        if remaining.is_zero() {
            continue;
        }
        let response = match channel.recv_sip_fresh(remaining).await {
            Ok(response) => response,
            Err(error) if udp_retransmit && is_register_read_retryable(&error) => {
                // Connected UDP sockets may surface an ICMP error immediately
                // from the previous datagram.  Treat that as a lost receive
                // attempt, not as a completed transaction failure.  Do not
                // spin on the same socket error: wait until RFC 3261 Timer E
                // schedules the identical REGISTER retransmission.
                let wait_until = deadline.min(next_retransmit);
                let wait = wait_until.saturating_duration_since(tokio::time::Instant::now());
                tracing::debug!(
                    error = error.code(),
                    wait_ms = wait.as_millis(),
                    "IMS REGISTER receive error; waiting for UDP retransmission"
                );
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                continue;
            }
            Err(error) => return Err(preserve_flow_error(error, receive_error)),
        };
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

fn is_register_read_retryable(error: &ImsError) -> bool {
    matches!(
        error.code(),
        "ims_channel_read_timeout"
            | "cellular_ims_channel_read_timeout"
            | "ims_register_read_timeout"
            | "ims_channel_read_failed"
            | "cellular_ims_channel_read_retryable"
    )
}

/// Parse the `Min-Expires` header from a `423 Interval Too Brief` response.
///
/// Read exactly one positive decimal floor. Duplicates (even identical), zero,
/// signed/comma values and integer overflow are invalid. A representable value
/// above the supported cap is explicitly unsupported; never round it down.
pub(crate) fn parse_min_expires(response: &[u8]) -> Result<u32, RegisterMinExpiresFailure> {
    let values = sip_frame::header_values(response, "Min-Expires");
    if values.len() != 1 {
        return Err(RegisterMinExpiresFailure::Invalid);
    }
    let value = values[0].trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(RegisterMinExpiresFailure::Invalid);
    }
    let parsed = value.parse::<u32>().map_err(|_| RegisterMinExpiresFailure::Invalid)?;
    if parsed == 0 {
        Err(RegisterMinExpiresFailure::Invalid)
    } else if parsed > MIN_EXPIRES_CAP {
        Err(RegisterMinExpiresFailure::Unsupported)
    } else {
        Ok(parsed)
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

    struct ResyncGuardAuthenticator { resync: bool }

    impl RegisterAuthenticator<FakeChannel> for ResyncGuardAuthenticator {
        fn validate_success(&self, _response: &[u8]) -> Result<(), ImsError> {
            if self.resync { Err(ImsError::new("ims_aka_resync_unexpected_status")) } else { Ok(()) }
        }
        async fn authenticated_request(&mut self, _response: &[u8], _cseq: u32) -> Result<Vec<u8>, ImsError> {
            self.resync = true;
            Ok(b"REGISTER AUTS proof-is-empty".to_vec())
        }
    }

    #[tokio::test]
    async fn final_success_validation_precedes_register_and_unregister_commit() {
        for unregister in [false, true] {
            let mut channel = FakeChannel {
                responses: VecDeque::from([response(401, "Unauthorized"), response(200, "OK")]),
                requeued: VecDeque::new(), sends: Vec::new(), transport: SipTransport::Tcp,
            };
            let mut auth = ResyncGuardAuthenticator { resync: false };
            if unregister {
                assert_eq!(run_unregister(&mut channel, b"REGISTER Expires 0", &mut auth).await, UnregisterResult::Rejected);
            } else {
                let failure = run_register_observed(&mut channel, b"REGISTER initial", &mut auth).await.unwrap_err();
                assert_eq!(failure.error.code(), "ims_aka_resync_unexpected_status");
                assert_eq!(failure.auth_rounds, 1);
            }
            assert_eq!(channel.sends.len(), 2);
        }
    }

    #[test]
    fn stream_security_and_framing_errors_are_not_silent_transport_hints() {
        for reason in ["cellular_ims_tls_handshake_failed", "cellular_ims_tls_authority_invalid",
            "cellular_ims_channel_stream_frame_invalid", "cellular_ims_channel_stream_response_owner_missing",
            "cellular_ims_channel_stream_budget_exhausted"]
        {
            let error = preserve_flow_error(ImsError::new(reason), "ims_register_initial_receive_failed");
            assert_eq!(error.code(), reason);
            let failure = RegisterFailure { error, response: None, auth_rounds: 0 };
            assert!(!failure.metadata().is_temporary());
            assert!(!failure.metadata().permits_variant_fallback());
        }
    }

    struct RequestContextGuard;

    impl RegisterAuthenticator<FakeChannel> for RequestContextGuard {
        fn validate_request(&self, request: &[u8]) -> Result<(), ImsError> {
            if request.ends_with(b"invalid-context") {
                Err(ImsError::new("digest_request_auth_mismatch"))
            } else {
                Ok(())
            }
        }

        async fn authenticated_request(&mut self, _: &[u8], _: u32) -> Result<Vec<u8>, ImsError> {
            Ok(b"REGISTER invalid-context".to_vec())
        }

        async fn rebuild_register_with_min_expires(
            &mut self, _: &[u8], _: u32, _: u32, _: bool,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(b"REGISTER invalid-context".to_vec())
        }
    }

    #[tokio::test]
    async fn request_context_is_checked_before_initial_auth_and_423_sends() {
        for status in [200, 401, 423] {
            let mut channel = FakeChannel {
                responses: VecDeque::from([response_with_header(status, "Test", "Min-Expires: 3600\r\n")]),
                requeued: VecDeque::new(), sends: Vec::new(), transport: SipTransport::Tcp,
            };
            let initial: &[u8] = if status == 200 { b"REGISTER invalid-context" } else { b"REGISTER initial" };
            let failure = run_register_observed(&mut channel, initial, &mut RequestContextGuard)
                .await.unwrap_err();
            assert_eq!(failure.error.code(), "digest_request_auth_mismatch");
            assert_eq!(channel.sends.len(), usize::from(status != 200));
            assert_eq!(failure.auth_rounds, u8::from(status == 401));
            assert!(!failure.metadata().permits_variant_fallback());
        }
    }

    struct ResponseProofGuard;

    impl RegisterAuthenticator<FakeChannel> for ResponseProofGuard {
        fn validate_success(&self, _: &[u8]) -> Result<(), ImsError> {
            Err(ImsError::new("digest_response_auth_mismatch"))
        }

        async fn authenticated_request(&mut self, _: &[u8], _: u32) -> Result<Vec<u8>, ImsError> {
            Ok(b"REGISTER authenticated".to_vec())
        }
    }

    #[tokio::test]
    async fn invalid_rspauth_is_authentication_failure_not_success_or_fallback() {
        for challenged in [false, true] {
            for unregister in [false, true] {
                let mut responses = VecDeque::new();
                if challenged { responses.push_back(response(401, "Unauthorized")); }
                responses.push_back(response(200, "OK"));
                let mut channel = FakeChannel {
                    responses, requeued: VecDeque::new(), sends: Vec::new(), transport: SipTransport::Tcp,
                };
                if unregister {
                    assert_eq!(run_unregister(&mut channel, b"REGISTER removal", &mut ResponseProofGuard).await,
                        UnregisterResult::Rejected);
                } else {
                    let failure = run_register_observed(&mut channel, b"REGISTER initial", &mut ResponseProofGuard)
                        .await.unwrap_err();
                    let metadata = failure.metadata();
                    assert_eq!(metadata.sip_status, Some(200));
                    assert_eq!(metadata.kind, RegisterFailureKind::AuthenticationRejected);
                    assert!(!metadata.is_temporary() && !metadata.permits_variant_fallback());
                    assert_eq!(super::super::registration::RegistrationLossReason::from_register_failure(&failure),
                        super::super::registration::RegistrationLossReason::AuthenticationRejected);
                }
                assert_eq!(channel.sends.len(), if challenged { 2 } else { 1 });
            }
        }
    }

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

    struct RetransmittingChannel {
        sends: Vec<Vec<u8>>,
        timed_out: bool,
    }

    impl ImsChannel for RetransmittingChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sends.push(frame.to_vec());
            Ok(())
        }

        async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            self.recv_sip_fresh(Duration::from_secs(1)).await
        }

        async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
            if !self.timed_out {
                // Make the first read consume the transaction's first 500 ms
                // receive window. The real UDP socket returns the same timeout
                // after that window, which is what causes Timer E to fire the
                // first retransmission before the response below is exposed.
                tokio::time::sleep(timeout).await;
                self.timed_out = true;
                return Err(ImsError::new("ims_channel_read_timeout"));
            }
            Ok(response_with_header(
                200,
                "OK",
                "Call-ID: refresh@dev\r\nCSeq: 7 REGISTER\r\n",
            ))
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

    struct ImmediateReadErrorChannel {
        sends: Vec<Vec<u8>>,
        read_attempts: u8,
    }

    impl ImsChannel for ImmediateReadErrorChannel {
        async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
            self.sends.push(frame.to_vec());
            Ok(())
        }

        async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
            self.recv_sip_fresh(timeout).await
        }

        async fn recv_sip_fresh(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
            if self.read_attempts == 0 {
                self.read_attempts = 1;
                return Err(ImsError::new("cellular_ims_channel_read_retryable"));
            }
            Ok(response_with_header(
                200,
                "OK",
                "Call-ID: refresh@dev\r\nCSeq: 7 REGISTER\r\n",
            ))
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

    #[tokio::test]
    async fn immediate_udp_read_error_waits_for_same_transaction_retransmission() {
        let request = b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: refresh@dev\r\nCSeq: 7 REGISTER\r\nContent-Length: 0\r\n\r\n";
        let mut channel = ImmediateReadErrorChannel {
            sends: Vec::new(),
            read_attempts: 0,
        };

        let result = run_register(&mut channel, request, &mut FakeAuthenticator)
            .await
            .expect("transient UDP read error should not abort REGISTER");

        assert_eq!(result.auth_rounds, 0);
        assert_eq!(channel.sends.len(), 2);
        assert_eq!(channel.sends[0], request);
        assert_eq!(channel.sends[1], request);
    }

    impl RegisterAuthenticator<ImmediateReadErrorChannel> for FakeAuthenticator {
        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(format!(
                "REGISTER sip:ims.example SIP/2.0\r\nCSeq: {cseq} REGISTER\r\nContent-Length: 0\r\n\r\n"
            )
            .into_bytes())
        }
    }

    impl RegisterAuthenticator<RetransmittingChannel> for FakeAuthenticator {
        async fn authenticated_request(
            &mut self,
            _challenge_response: &[u8],
            cseq: u32,
        ) -> Result<Vec<u8>, ImsError> {
            Ok(format!(
                "REGISTER sip:ims.example SIP/2.0\r\nCSeq: {cseq} REGISTER\r\nContent-Length: 0\r\n\r\n"
            )
            .into_bytes())
        }
    }

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
    async fn udp_register_retransmits_the_same_transaction_after_a_timeout() {
        let request = b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: refresh@dev\r\nCSeq: 7 REGISTER\r\nContent-Length: 0\r\n\r\n";
        let mut channel = RetransmittingChannel {
            sends: Vec::new(),
            timed_out: false,
        };

        run_register(&mut channel, request, &mut FakeAuthenticator)
            .await
            .unwrap();

        assert_eq!(channel.sends.len(), 2);
        assert_eq!(channel.sends[0], channel.sends[1]);
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
    async fn unregister_never_rebuilds_a_positive_lease_after_423() {
        for challenged in [false, true] {
            let mut responses = VecDeque::new();
            if challenged {
                responses.push_back(response(401, "Unauthorized"));
            }
            responses.push_back(response_with_header(
                423,
                "Interval Too Brief",
                "Min-Expires: 3600\r\n",
            ));
            responses.push_back(response(200, "OK"));
            let mut channel = FakeChannel {
                responses,
                sends: Vec::new(),
                requeued: VecDeque::new(),
                transport: SipTransport::Udp,
            };
            let mut auth = MinExpiresAuthenticator {
                rebuilds: Vec::new(),
            };
            assert_eq!(
                run_unregister(
                    &mut channel,
                    b"REGISTER sip:ims.example SIP/2.0\r\nExpires: 0\r\n\r\n",
                    &mut auth,
                )
                .await,
                UnregisterResult::Rejected,
            );
            assert!(auth.rebuilds.is_empty());
            assert_eq!(channel.sends.len(), if challenged { 2 } else { 1 });
            assert_eq!(channel.responses.len(), 1);
        }
    }

    #[tokio::test]
    async fn unregister_does_not_confirm_rejection_or_transport_loss() {
        for (status, reason) in [(403, "Forbidden"), (500, "Server Internal Error")] {
            let mut rejected = FakeChannel {
                responses: VecDeque::from([response(status, reason)]),
                sends: Vec::new(),
                requeued: VecDeque::new(),
                transport: SipTransport::Udp,
            };
            assert_eq!(
                run_unregister(&mut rejected, b"unregister", &mut FakeAuthenticator).await,
                UnregisterResult::Rejected
            );
        }

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

    #[test]
    fn register_transaction_rejects_trailing_cseq_fields() {
        let key = RegisterTransactionKey::from_register_request(
            b"REGISTER sip:ims.example SIP/2.0\r\nCall-ID: abc\r\nCSeq: 7 REGISTER\r\n\r\n"
        ).unwrap();
        assert!(!key.matches_response(b"SIP/2.0 421 X\r\nCall-ID: abc\r\nCSeq: 7 REGISTER private-sentinel\r\n\r\n"));
        assert!(key.matches_response(b"SIP/2.0 421 X\r\nCall-ID: abc\r\nCSeq: 7 REGISTER\r\n\r\n"));
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

    #[test]
    fn min_expires_parser_rejects_missing_duplicate_zero_overflow_and_cap() {
        for headers in [
            "", "Min-Expires: 0\r\n", "Min-Expires: +600\r\n",
            "Min-Expires: -1\r\n", "Min-Expires: 600, 1200\r\n",
            "Min-Expires: 4294967296\r\n", "Min-Expires: \r\n",
            "Min-Expires: 600\r\nMin-Expires: 600\r\n",
            "Min-Expires: 600\r\nmIn-eXpIrEs: 1200\r\n",
        ] {
            assert_eq!(parse_min_expires(&response_with_header(423, "Interval Too Brief", headers)),
                Err(RegisterMinExpiresFailure::Invalid), "{headers:?}");
        }
        for floor in [MIN_EXPIRES_CAP + 1, u32::MAX] {
            assert_eq!(parse_min_expires(&response_with_header(423, "Interval Too Brief",
                &format!("Min-Expires: {floor}\r\n"))), Err(RegisterMinExpiresFailure::Unsupported));
        }
        assert_eq!(parse_min_expires(&response_with_header(423, "Interval Too Brief",
            "Min-Expires: 86400\r\n")), Ok(MIN_EXPIRES_CAP));
    }

    #[tokio::test]
    async fn min_expires_invalid_and_unsupported_stop_without_retry() {
        for (headers, reason) in [
            ("", RegisterMinExpiresFailure::Invalid),
            ("Min-Expires: 0\r\n", RegisterMinExpiresFailure::Invalid),
            ("Min-Expires: 4294967296\r\n", RegisterMinExpiresFailure::Invalid),
            ("Min-Expires: 600\r\nMin-Expires: 600\r\n", RegisterMinExpiresFailure::Invalid),
            ("Min-Expires: 86401\r\n", RegisterMinExpiresFailure::Unsupported),
        ] {
            let mut channel = FakeChannel {
                responses: VecDeque::from([response_with_header(423, "Interval Too Brief", headers)]),
                sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
            };
            let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
            let mut budget = RegisterNegotiationBudget::default();
            let failure = run_register_observed_with_budget(&mut channel, b"REGISTER initial", &mut auth, &mut budget)
                .await.unwrap_err();
            assert_eq!(failure.error, reason.error(false));
            assert_eq!(budget.failure_metadata(&failure).min_expires, Some(reason));
            assert!(!budget.permits_candidate_expansion());
            assert_eq!(channel.sends.len(), 1);
            assert!(auth.rebuilds.is_empty());
        }
    }

    #[tokio::test]
    async fn min_expires_exhaustion_closes_cross_candidate_budget_before_send() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 1200\r\n"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 1800\r\n"),
                response(200, "OK"),
            ]),
            sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
        };
        let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
        let mut budget = RegisterNegotiationBudget::default();
        for _candidate in 0..3 {
            let failure = run_register_observed_with_budget(&mut channel, b"REGISTER shape", &mut auth, &mut budget)
                .await.unwrap_err();
            assert_eq!(failure.error.code(), "ims_register_initial_min_expires_exhausted");
            assert_eq!(budget.failure_metadata(&failure).min_expires, Some(RegisterMinExpiresFailure::Exhausted));
            assert_eq!(channel.sends.len(), 3);
        }
        assert_eq!(auth.rebuilds, [(600, false), (1200, false)]);
        assert_eq!(channel.responses.len(), 1);
    }

    #[tokio::test]
    async fn min_expires_then_400_or_transport_cannot_reopen_candidates() {
        for next_status in [Some(400), None] {
            let mut responses = VecDeque::from([
                response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
            ]);
            if let Some(status) = next_status {
                responses.push_back(response(status, "Bad Request"));
            }
            let mut channel = FakeChannel {
                responses, sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
            };
            let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
            let mut budget = RegisterNegotiationBudget::default();
            let failure = run_register_observed_with_budget(&mut channel, b"REGISTER shape", &mut auth, &mut budget)
                .await.unwrap_err();
            let metadata = budget.failure_metadata(&failure);
            assert_eq!(metadata.stage, RegisterFailureStage::Initial);
            assert_eq!(metadata.min_expires, Some(RegisterMinExpiresFailure::Negotiated));
            assert_eq!(metadata.kind, if next_status.is_some() {
                RegisterFailureKind::Negotiation
            } else { RegisterFailureKind::Transport });
            assert_eq!(metadata.sip_status, next_status.or(Some(423)));
            assert!(!metadata.permits_variant_fallback());
            assert!(!budget.permits_candidate_expansion());
            assert!(run_register_observed_with_budget(&mut channel, b"REGISTER another shape", &mut auth, &mut budget)
                .await.is_err());
            assert_eq!(channel.sends.len(), 2);
        }
    }

    #[tokio::test]
    async fn min_expires_budget_spans_authentication_without_resetting_auth_limit() {
        for exhausted_auth in [false, true] {
            let mut channel = FakeChannel {
                responses: if exhausted_auth {
                    VecDeque::from([
                        response(401, "Unauthorized"),
                        response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                        response(401, "Unauthorized"), response(407, "Proxy Authentication Required"),
                    ])
                } else {
                    VecDeque::from([
                        response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                        response(401, "Unauthorized"),
                        response_with_header(423, "Interval Too Brief", "Min-Expires: 1200\r\n"),
                        response(200, "OK"),
                    ])
                },
                sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
            };
            let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
            let mut budget = RegisterNegotiationBudget::default();
            let result = run_register_observed_with_budget(&mut channel, b"REGISTER initial", &mut auth, &mut budget).await;
            if exhausted_auth {
                let failure = result.unwrap_err();
                assert_eq!(failure.auth_rounds, 2);
                assert_eq!(failure.error.code(), "ims_register_auth_rejected");
                assert_eq!(budget.failure_metadata(&failure).stage, RegisterFailureStage::Authenticated);
            } else {
                let success = result.unwrap();
                assert!(success.authenticated);
                assert_eq!(success.auth_rounds, 1);
                assert_eq!(auth.rebuilds, [(600, false), (1200, true)]);
                assert_eq!(channel.sends[3], b"REGISTER 4 Expires 1200");
            }
            assert_eq!(channel.sends.len(), 4);
            assert!(!budget.permits_candidate_expansion());
        }
    }

    #[tokio::test]
    async fn min_expires_repeated_floor_is_invalid_not_a_fresh_budget() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([
                response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
                response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"),
            ]),
            sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
        };
        let failure = run_register_observed(&mut channel, b"REGISTER initial",
            &mut MinExpiresAuthenticator { rebuilds: Vec::new() }).await.unwrap_err();
        assert_eq!(failure.error.code(), "ims_register_initial_min_expires_invalid");
        assert_eq!(channel.sends.len(), 2);
    }

    #[tokio::test]
    async fn zero_expiry_register_api_also_refuses_positive_lease() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n")]),
            sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
        };
        let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
        let failure = run_register_observed(&mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nExpires: 0\r\n\r\n", &mut auth).await.unwrap_err();
        assert_eq!(failure.error.code(), "ims_register_initial_min_expires_unsupported");
        assert!(auth.rebuilds.is_empty());
        assert_eq!(channel.sends.len(), 1);
    }

    #[tokio::test]
    async fn min_expires_default_rebuilder_is_explicitly_unsupported() {
        for authenticated in [false, true] {
            let mut responses = VecDeque::new();
            if authenticated {
                responses.push_back(response(401, "Unauthorized"));
            }
            responses.push_back(response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n"));
            let mut channel = FakeChannel {
                responses, sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
            };
            let failure = run_register_observed(&mut channel, b"REGISTER initial", &mut FakeAuthenticator)
                .await.unwrap_err();
            assert_eq!(failure.error, RegisterMinExpiresFailure::Unsupported.error(authenticated));
            assert_eq!(channel.sends.len(), if authenticated { 2 } else { 1 });
        }
    }

    #[tokio::test]
    async fn min_expires_must_increase_the_current_requested_expiry() {
        let mut channel = FakeChannel {
            responses: VecDeque::from([response_with_header(423, "Interval Too Brief", "Min-Expires: 600\r\n")]),
            sends: Vec::new(), requeued: VecDeque::new(), transport: SipTransport::Tcp,
        };
        let mut auth = MinExpiresAuthenticator { rebuilds: Vec::new() };
        let failure = run_register_observed(&mut channel,
            b"REGISTER sip:ims.example SIP/2.0\r\nExpires: 1200\r\n\r\n", &mut auth).await.unwrap_err();
        assert_eq!(failure.error.code(), "ims_register_initial_min_expires_invalid");
        assert!(auth.rebuilds.is_empty());
        assert_eq!(channel.sends.len(), 1);
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
