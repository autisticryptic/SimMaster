//! Owned ARC RPC session for verified codecs, not a network/provider backend.
//!
//! Original selected session id, imported phone token (`pwd`) and returned ARC
//! auth token (`token`) are distinct. Caller supplies verified routing metadata
//! and decoded deployment properties; this module never guesses endpoints,
//! decrypts ENC values, resolves addresses, takes over a device or grants channels.
use super::{
    batch_scheduler::{BatchError, BatchScheduler, ScheduledBatch},
    controller::Callback,
    rpc_batch::{decode_swap_calls_reply, encode_swap_calls, BatchPayload, BatchReply, SWAP_CALLS_METHOD},
    rpc_bootstrap::{decode_get_app_configs_result, encode_get_app_configs, AppConfigsResult, GET_APP_CONFIGS_METHOD},
    rpc_envelope::{decode_reply, encode_request},
    rpc_login::{decode_login_session_result, encode_login_session, normalize_session_identity, LOGIN_SESSION_METHOD},
    rpc_pending::{HostingRpcTracker, OwnedRpcCallback, RpcConnectionScope},
    rpc_stream::{CodecError, RpcBytes, RpcProperty, MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
    session::{AttemptOwner, BackendFailure},
};
use std::{collections::BTreeMap, fmt, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcPhase { Initial, LoginPending, Authenticated, Closed }

/// All references/values are captured from the original SDK configuration.
/// Requiring a selected id is deliberate: this adapter does not invent the
/// Android library's built-in deployment defaults or force-login behaviour.
pub struct ArcLoginInputs<'a> {
    pub typed_identity: &'a [u8],
    pub domain_name: &'a [u8],
    pub selected_session_id: &'a [u8],
    pub phone_login_token: &'a [u8],
    pub application_key: &'a [u8],
    pub domain_id: &'a [u8],
    pub application_id: &'a [u8],
    pub device_info_cc: &'a [u8],
    pub client_properties: &'a [RpcProperty<'a>],
}

impl fmt::Debug for ArcLoginInputs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ArcLoginInputs(<redacted>)") }
}

struct ArcAuthentication { user_id: Vec<u8>, selected_session_id: Vec<u8>, token: Vec<u8> }

#[derive(Clone, Copy)]
enum PendingKind { Bootstrap, Login, Swap }

struct PendingRpc {
    kind: PendingKind, selected_session_id: Option<Vec<u8>>, scheduler: Option<Arc<()>>,
}

/// Input bytes cannot become callbacks for a different owner or connection.
/// Missing/retired ids are ignored, allocation never reuses a call id.
pub struct ArcRpcSession {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    phase: ArcPhase,
    next_id: Option<i32>,
    pending: BTreeMap<i32, PendingRpc>,
    authentication: Option<ArcAuthentication>,
    hosting_tracker_taken: bool,
    batch_scheduler_taken: bool,
}

impl fmt::Debug for ArcRpcSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArcRpcSession").field("phase", &self.phase)
            .field("pending_count", &self.pending.len()).field("material", &"<redacted>").finish()
    }
}

/// Successfully matched outer batch with its original ARC provenance. The
/// fields cannot be reconstructed or rebound by a reply consumer. A closed ARC
/// scope also invalidates replies retained by offline/mock callers.
pub struct OwnedBatchReply<'a> {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    call_id: i32,
    reply: BatchReply<'a>,
    scheduler: Option<Arc<()>>,
}

impl<'a> OwnedBatchReply<'a> {
    pub fn owner(&self) -> &AttemptOwner { &self.owner }
    pub fn call_id(&self) -> i32 { self.call_id }
    pub fn reply(&self) -> &BatchReply<'a> { &self.reply }
    pub(super) fn is_scheduled(&self) -> bool { self.scheduler.is_some() }
    pub(super) fn matches_scheduler(&self, scheduler: &Arc<()>) -> bool {
        self.scheduler.as_ref().is_some_and(|original| Arc::ptr_eq(original, scheduler))
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(connection) && self.connection.is_open()
    }
}

impl fmt::Debug for OwnedBatchReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedBatchReply(<redacted>)") }
}

pub enum ArcReply<'a> {
    Bootstrap(AppConfigsResult<'a>),
    Login(OwnedRpcCallback),
    Swap(OwnedBatchReply<'a>),
    Failure(OwnedRpcCallback),
}

impl fmt::Debug for ArcReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ArcReply(<redacted>)") }
}

impl ArcRpcSession {
    pub fn new(owner: AttemptOwner) -> Self {
        Self { owner, connection: RpcConnectionScope::fresh(), phase: ArcPhase::Initial,
            next_id: Some(1), pending: BTreeMap::new(), authentication: None, hosting_tracker_taken: false,
            batch_scheduler_taken: false }
    }

    pub fn connection_scope(&self) -> RpcConnectionScope { self.connection.clone() }
    pub fn phase(&self) -> ArcPhase {
        if self.connection.is_open() { self.phase } else { ArcPhase::Closed }
    }
    pub(super) fn owner(&self) -> &AttemptOwner { &self.owner }

    /// One tracker per ARC incarnation keeps nested sequences unique and binds
    /// owner/connection at construction, never when a batch arrives.
    pub fn take_hosting_tracker(&mut self) -> Result<HostingRpcTracker, CodecError> {
        if self.phase() == ArcPhase::Closed || self.hosting_tracker_taken { return Err(CodecError::InvalidField); }
        self.hosting_tracker_taken = true;
        Ok(HostingRpcTracker::for_arc_session(self.owner.clone(), self.connection.clone()))
    }

    pub fn incoming_dispatcher(&self, binding: super::rpc_incoming::IncomingBinding<'_>)
        -> Result<super::rpc_incoming::IncomingDispatcher, CodecError> {
        if self.phase() != ArcPhase::Authenticated { return Err(CodecError::InvalidField); }
        super::rpc_incoming::IncomingDispatcher::for_arc_session(self.owner.clone(), self.connection.clone(), binding)
    }

    /// Enable exactly one owned scheduler before any raw swap submission. The
    /// nonzero incarnation is generated/imported locally, never a credential or
    /// endpoint default. Configuring this engine performs no network operation.
    pub fn take_batch_scheduler(&mut self, incarnation: u32) -> Result<BatchScheduler, BatchError> {
        if self.phase() != ArcPhase::Authenticated || self.batch_scheduler_taken
            || self.pending.values().any(|p| matches!(p.kind, PendingKind::Swap)) { return Err(BatchError::Scope); }
        let scheduler = BatchScheduler::for_arc_session(self.owner.clone(), self.connection.clone(), incarnation)?;
        self.batch_scheduler_taken = true;
        Ok(scheduler)
    }

    fn allocate(&mut self, metadata: &[RpcProperty<'_>], method: &[u8], arguments: &[u8],
        pending: PendingRpc) -> Result<(i32, RpcBytes), CodecError> {
        if self.phase() == ArcPhase::Closed { return Err(CodecError::InvalidField); }
        if self.pending.len() >= MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let id = self.next_id.ok_or(CodecError::InvalidCallId)?;
        let bytes = encode_request(id, metadata, method, arguments)?;
        self.next_id = id.checked_add(1);
        self.pending.insert(id, pending);
        Ok((id, bytes))
    }

    /// Must still travel over the verified trusted lower-layer connection.
    /// Routing keys id/domain/__magic are supplied, not parsed/derived here.
    pub fn prepare_bootstrap(&mut self, application_key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
        if self.phase != ArcPhase::Initial || !self.pending.is_empty() { return Err(CodecError::InvalidField); }
        reject_auth_keys(routing)?;
        let arguments = encode_get_app_configs(application_key)?;
        self.allocate(routing, GET_APP_CONFIGS_METHOD.as_bytes(), arguments.expose_to_transport(),
            PendingRpc { kind: PendingKind::Bootstrap, selected_session_id: None, scheduler: None })
    }

    /// The imported phone token is per-call metadata `pwd`, not a method string
    /// and not SipPwd. client-properties are copied, then native keys override.
    /// ForceFlag=1 is explicitly rejected: do not inherit Android's forced
    /// takeover just because its DEX uses Mtc_CliDbSetForceLogin(true).
    pub fn prepare_login(&mut self, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
        if self.phase != ArcPhase::Initial || !self.pending.is_empty() { return Err(CodecError::InvalidField); }
        reject_auth_keys(routing)?;
        validate_bytes(inputs.typed_identity, true)?;
        validate_bytes(inputs.domain_name, true)?;
        validate_bytes(inputs.selected_session_id, true)?;
        if inputs.selected_session_id.len() > 127 { return Err(CodecError::TooLarge); }
        validate_bytes(inputs.phone_login_token, true)?;
        validate_bytes(inputs.application_key, true)?;
        validate_bytes(inputs.domain_id, true)?;
        validate_bytes(inputs.application_id, true)?;
        validate_bytes(inputs.device_info_cc, false)?;
        validate_properties(inputs.client_properties)?;
        if inputs.client_properties.iter().any(|field| {
            (field.key == b"ForceFlag" && field.value != b"0") || field.key == b"pwd" || field.key == b"token"
        }) { return Err(CodecError::InvalidField); }
        let identity = normalize_session_identity(inputs.typed_identity, inputs.domain_name)?;
        let mut properties = inputs.client_properties.to_vec();
        if !properties.iter().any(|field| field.key == b"ForceFlag") {
            properties.push(RpcProperty { key: b"ForceFlag", value: b"0" });
        }
        for (key, value) in [(b"__appKey".as_slice(), inputs.application_key),
            (b"__domain".as_slice(), inputs.domain_id), (b"__app".as_slice(), inputs.application_id),
            (b"DeviceInfo.CC".as_slice(), inputs.device_info_cc)] {
            properties.retain(|field| field.key != key);
            properties.push(RpcProperty { key, value });
        }
        let arguments = encode_login_session(identity.expose_to_backend(), inputs.selected_session_id, &properties)?;
        let mut metadata = routing.to_vec();
        metadata.push(RpcProperty { key: b"pwd", value: inputs.phone_login_token });
        let selected_session_id = inputs.selected_session_id.to_vec();
        let prepared = self.allocate(&metadata, LOGIN_SESSION_METHOD.as_bytes(), arguments.expose_to_transport(),
            PendingRpc { kind: PendingKind::Login, selected_session_id: Some(selected_session_id), scheduler: None })?;
        self.phase = ArcPhase::LoginPending;
        Ok(prepared)
    }

    pub fn prepare_swap_calls(&mut self, payload: &BatchPayload<'_>, routing: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
        if self.batch_scheduler_taken { return Err(CodecError::InvalidField); }
        self.prepare_swap(payload, routing, None)
    }

    pub fn prepare_scheduled_swap(&mut self, batch: &ScheduledBatch, routing: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
        if !self.batch_scheduler_taken || !batch.matches(&self.owner, &self.connection) || batch.is_encoded() {
            return Err(CodecError::InvalidField);
        }
        let result = self.prepare_swap(&batch.payload(), routing, Some(batch.identity()))?;
        batch.mark_encoded();
        Ok(result)
    }

    fn prepare_swap(&mut self, payload: &BatchPayload<'_>, routing: &[RpcProperty<'_>], scheduler: Option<Arc<()>>)
        -> Result<(i32, RpcBytes), CodecError> {
        if self.phase() != ArcPhase::Authenticated { return Err(CodecError::InvalidField); }
        if scheduler.is_some() && payload.fields[0] == 0 { return Err(CodecError::InvalidField); }
        reject_auth_keys(routing)?;
        let auth = self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        let arguments = encode_swap_calls(&auth.user_id, &auth.selected_session_id, payload)?;
        let mut metadata = routing.to_vec();
        metadata.push(RpcProperty { key: b"token", value: &auth.token });
        let id = self.next_id.ok_or(CodecError::InvalidCallId)?;
        if self.pending.len() >= MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let bytes = encode_request(id, &metadata, SWAP_CALLS_METHOD.as_bytes(), arguments.expose_to_transport())?;
        self.next_id = id.checked_add(1);
        self.pending.insert(id, PendingRpc { kind: PendingKind::Swap, selected_session_id: None, scheduler });
        Ok((id, bytes))
    }

    /// Retire timeouts explicitly. A submitted login may have changed remote
    /// state, so cancelling it closes this incarnation rather than reopening it.
    pub fn retire_request(&mut self, id: i32) {
        if self.pending.remove(&id).is_some_and(|pending| matches!(pending.kind, PendingKind::Login)) {
            self.close();
        }
    }

    pub fn close(&mut self) {
        self.phase = ArcPhase::Closed;
        self.connection.close();
        self.authentication = None;
        self.pending.clear();
    }

    pub fn accepts_scope(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        self.phase != ArcPhase::Closed && &self.owner == owner
            && self.connection.matches(connection) && self.connection.is_open()
    }

    fn failure(&mut self) -> ArcReply<'static> {
        self.close();
        ArcReply::Failure(OwnedRpcCallback { owner: self.owner.clone(),
            callback: Callback::Failure(BackendFailure::AuthenticationRejected) })
    }

    /// Caller delivers already-authenticated, demultiplexed reply bytes. Only a
    /// successful matched login emits SdkLogin, never hosting/channels readiness.
    /// A failed/false batch emits no nested results and closes this auth scope.
    pub fn on_reply<'a>(&mut self, owner: &AttemptOwner, connection: &RpcConnectionScope, bytes: &'a [u8])
        -> Result<Option<ArcReply<'a>>, CodecError> {
        if !self.accepts_scope(owner, connection) { return Ok(None); }
        let reply = match decode_reply(bytes) {
            Ok(reply) => reply,
            Err(error) => { self.close(); return Err(error); }
        };
        let Some(pending) = self.pending.remove(&reply.call_id) else { return Ok(None); };
        let result = match pending.kind {
            PendingKind::Bootstrap => match decode_get_app_configs_result(reply.result_code, reply.result_stream) {
                Ok(result) => ArcReply::Bootstrap(result),
                Err(_) => self.failure(),
            },
            PendingKind::Login => match decode_login_session_result(reply.result_code, reply.result_stream) {
                Ok(result) if self.phase == ArcPhase::LoginPending => {
                    let Some(selected_session_id) = pending.selected_session_id else { return Ok(Some(self.failure())); };
                    self.authentication = Some(ArcAuthentication { user_id: result.user_id.to_vec(),
                        selected_session_id, token: result.session_token.to_vec() });
                    self.phase = ArcPhase::Authenticated;
                    ArcReply::Login(OwnedRpcCallback { owner: self.owner.clone(), callback: Callback::SdkLogin { success: true } })
                }
                _ => self.failure(),
            },
            PendingKind::Swap => match decode_swap_calls_reply(reply.result_code, reply.result_stream) {
                Ok(result) if result.success => ArcReply::Swap(OwnedBatchReply {
                    owner: self.owner.clone(), connection: self.connection.clone(), call_id: reply.call_id, reply: result,
                    scheduler: pending.scheduler,
                }),
                _ => self.failure(),
            },
        };
        Ok(Some(result))
    }
}

impl Drop for ArcRpcSession {
    fn drop(&mut self) { self.close(); }
}

fn validate_bytes(bytes: &[u8], required: bool) -> Result<(), CodecError> {
    if (required && bytes.is_empty()) || bytes.iter().any(|byte| byte.is_ascii_control())
        || bytes.windows(4).any(|value| value == b"ENC(") { return Err(CodecError::InvalidField); }
    if bytes.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
    Ok(())
}

fn validate_properties(properties: &[RpcProperty<'_>]) -> Result<(), CodecError> {
    if properties.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    for (index, field) in properties.iter().enumerate() {
        if field.key.len() > MAX_FIELD_BYTES || field.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        if properties[..index].iter().any(|previous| previous.key == field.key) { return Err(CodecError::DuplicateKey); }
    }
    Ok(())
}

fn reject_auth_keys(properties: &[RpcProperty<'_>]) -> Result<(), CodecError> {
    validate_properties(properties)?;
    if properties.iter().any(|field| field.key == b"pwd" || field.key == b"token") {
        return Err(CodecError::InvalidField);
    }
    Ok(())
}

#[cfg(test)]
#[path = "arc_session_tests.rs"]
mod tests;
