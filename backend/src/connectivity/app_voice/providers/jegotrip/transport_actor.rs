//! Serialized original-IO/TLS/ARC runtime for caller-provided verified routing.
//!
//! Never dials, resolves, installs a provider or grants channels. The caller
//! supplies a nonblocking AsyncRead+AsyncWrite object and exact imported TLS/
//! route/login inputs. One actor owns that IO and all TLS/ARC state. Scope close
//! serializes with every write/flush poll; partial/cancelled writes terminate,
//! not replay or rebind. Bytes already accepted by IO/kernel cannot be recalled.
use super::{
    arc_session::{ArcLoginInputs, ArcPhase},
    call_binding::CallIncomingBinding,
    call_delivery::CallReplyTicket,
    call_control::{CallControlTicket,OwnedCallOperationReply},
    rpc_calls::CallOperation,
    offline_backend::{OfflineBackend, OfflineEvent},
    packet_transport::{OwnedWireFrames, PacketRoute, RouterTiming},
    rpc_incoming::IncomingBinding,
    rpc_pending::RpcConnectionScope,
    rpc_stream::RpcProperty,
    session::{AttemptOwner, GatewayRegistration},
    tls_engine::{TlsEngine, TlsError, TlsParameters, TlsPhase},
    batch_scheduler::ServerRequestTicket,
    arc_protocol::DirectReplyTicket,
    rpc_im::{ImQuery,ImOperation,OwnedImReply},
    im_inbox::ImReceiptGate,
    im_read_interest::{ImReadInterest,ImReadWait},
};
#[path="im_received_actor.rs"]
mod received;
use received::{ReceivedWork,ReceivedPending};
#[path="im_user_read_actor.rs"]
mod user_read;
use user_read::{UserReadWork,UserReadPending};
#[path="user_entry_actor.rs"]
mod user_entry;
use std::{fmt, future::poll_fn, io, pin::Pin, sync::Arc, task::Poll, time::{Duration, Instant}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite}, sync::{mpsc, oneshot}, task::JoinHandle};
use zeroize::{Zeroize, Zeroizing};

const MAX_COMMANDS: usize = 32;
const MAX_EVENTS: usize = 128;
const MAX_PARAMETER_BYTES: usize = 128 * 1024;
const MAX_PENDING_RPC: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError { Scope, InvalidConfiguration, Backpressure, Closed, Cancelled, Timeout, Io, Protocol, SessionRevoked, CallNotReady,CallRejected,UnregistrationRejected, LoggedOut, Tls(TlsError) }
impl From<TlsError> for RuntimeError { fn from(error: TlsError) -> Self { Self::Tls(error) } }

pub struct RuntimePolicy { pub io_timeout: Duration, pub rpc_timeout: Duration }
impl RuntimePolicy {
    fn valid(&self) -> bool {
        !self.io_timeout.is_zero() && self.io_timeout <= Duration::from_secs(30)
            && !self.rpc_timeout.is_zero() && self.rpc_timeout <= Duration::from_secs(60)
    }
}

struct Property { key: Vec<u8>, value: Vec<u8> }
impl Drop for Property { fn drop(&mut self) { self.key.zeroize(); self.value.zeroize(); } }
impl Property {
    fn capture(properties: &[RpcProperty<'_>]) -> Result<Vec<Self>, RuntimeError> {
        if properties.len() > super::rpc_stream::MAX_MAP_ENTRIES { return Err(RuntimeError::InvalidConfiguration); }
        let mut bytes = 0usize;
        for (index, property) in properties.iter().enumerate() {
            if property.key.len() > super::rpc_stream::MAX_FIELD_BYTES || property.value.len() > super::rpc_stream::MAX_FIELD_BYTES
                || properties[..index].iter().any(|prior| prior.key == property.key) { return Err(RuntimeError::InvalidConfiguration); }
            bytes = bytes.checked_add(property.key.len() + property.value.len()).ok_or(RuntimeError::InvalidConfiguration)?;
            if bytes > MAX_PARAMETER_BYTES { return Err(RuntimeError::InvalidConfiguration); }
        }
        Ok(properties.iter().map(|p| Self { key: p.key.to_vec(), value: p.value.to_vec() }).collect())
    }
}
fn logout_properties(properties: &[RpcProperty<'_>]) -> Result<Vec<Property>, RuntimeError> {
    if properties.iter().any(|p| p.key == b"pwd" || p.key == b"token") { return Err(RuntimeError::InvalidConfiguration); }
    Property::capture(properties)
}
fn borrowed(properties: &[Property]) -> Vec<RpcProperty<'_>> {
    properties.iter().map(|p| RpcProperty { key: &p.key, value: &p.value }).collect()
}

struct Login {
    identity: Vec<u8>, domain: Vec<u8>, selector: Vec<u8>, phone: Vec<u8>, key: Vec<u8>,
    domain_id: Vec<u8>, application_id: Vec<u8>, cc: Vec<u8>, properties: Vec<Property>,
}
impl Drop for Login {
    fn drop(&mut self) { self.identity.zeroize(); self.domain.zeroize(); self.selector.zeroize(); self.phone.zeroize(); self.key.zeroize();
        self.domain_id.zeroize(); self.application_id.zeroize(); self.cc.zeroize(); }
}
impl Login {
    fn capture(inputs: ArcLoginInputs<'_>) -> Result<Self, RuntimeError> {
        let values = [inputs.typed_identity, inputs.domain_name, inputs.selected_session_id, inputs.phone_login_token,
            inputs.application_key, inputs.domain_id, inputs.application_id, inputs.device_info_cc];
        let total = values.iter().try_fold(0usize, |n, v| n.checked_add(v.len())).ok_or(RuntimeError::InvalidConfiguration)?;
        let properties = Property::capture(inputs.client_properties)?;
        let property_bytes = properties.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>();
        if total.checked_add(property_bytes).is_none_or(|n| n > MAX_PARAMETER_BYTES) { return Err(RuntimeError::InvalidConfiguration); }
        for (index, value) in values.iter().enumerate() {
            if (index != 7 && value.is_empty()) || value.len() > super::rpc_stream::MAX_FIELD_BYTES
                || value.iter().any(u8::is_ascii_control) || value.windows(4).any(|v| v == b"ENC(") {
                return Err(RuntimeError::InvalidConfiguration);
            }
        }
        if inputs.selected_session_id.len() > 127 { return Err(RuntimeError::InvalidConfiguration); }
        Ok(Self { identity: inputs.typed_identity.to_vec(), domain: inputs.domain_name.to_vec(), selector: inputs.selected_session_id.to_vec(),
            phone: inputs.phone_login_token.to_vec(), key: inputs.application_key.to_vec(), domain_id: inputs.domain_id.to_vec(),
            application_id: inputs.application_id.to_vec(), cc: inputs.device_info_cc.to_vec(), properties })
    }
    fn inputs<'a>(&'a self, properties: &'a [RpcProperty<'a>]) -> ArcLoginInputs<'a> {
        ArcLoginInputs { typed_identity: &self.identity, domain_name: &self.domain, selected_session_id: &self.selector,
            phone_login_token: &self.phone, application_key: &self.key, domain_id: &self.domain_id,
            application_id: &self.application_id, device_info_cc: &self.cc, client_properties: properties }
    }
}
struct Binding { object: Vec<u8>, domain: Vec<u8>, receiver: Vec<u8> }
impl Drop for Binding { fn drop(&mut self) { self.object.zeroize(); self.domain.zeroize(); self.receiver.zeroize(); } }
impl Binding {
    fn capture(binding: IncomingBinding<'_>) -> Result<Self, RuntimeError> {
        let fields = [binding.object_id, binding.domain, binding.receiver_kind];
        let total = fields.iter().try_fold(0usize, |n, v| n.checked_add(v.len())).ok_or(RuntimeError::InvalidConfiguration)?;
        if total > MAX_PARAMETER_BYTES || fields.iter().any(|v| v.is_empty() || v.len() > super::rpc_stream::MAX_FIELD_BYTES
            || v.iter().any(u8::is_ascii_control) || v.windows(4).any(|w| w == b"ENC(")) || binding.receiver_kind.contains(&b'*') {
            return Err(RuntimeError::InvalidConfiguration);
        }
        Ok(Self { object: binding.object_id.to_vec(), domain: binding.domain.to_vec(), receiver: binding.receiver_kind.to_vec() })
    }
    fn borrowed(&self) -> IncomingBinding<'_> { IncomingBinding { object_id: &self.object, domain: &self.domain, receiver_kind: &self.receiver } }
}
struct CallBinding{object:Zeroizing<Vec<u8>>,domain:Zeroizing<Vec<u8>>}
impl CallBinding{
    fn capture(binding:CallIncomingBinding<'_>)->Result<Self,RuntimeError>{
        let values=[binding.object_id,binding.domain];let total=values.iter().map(|v|v.len()).sum::<usize>();
        if total>MAX_PARAMETER_BYTES||values.iter().any(|v|v.is_empty()||v.len()>super::rpc_stream::MAX_FIELD_BYTES||v.contains(&b'*')||v.iter().any(u8::is_ascii_control)||v.windows(4).any(|w|w==b"ENC(")){return Err(RuntimeError::InvalidConfiguration);}
        Ok(Self{object:Zeroizing::new(binding.object_id.to_vec()),domain:Zeroizing::new(binding.domain.to_vec())})
    }
    fn borrowed(&self)->CallIncomingBinding<'_>{CallIncomingBinding{object_id:&self.object,domain:&self.domain}}
}
struct Gateway { uri: String, status: i32, prefix: String, channel: String, int_code: String, number: String, password: String, metadata: Vec<Property> }
impl Drop for Gateway {
    fn drop(&mut self) { self.uri.zeroize(); self.prefix.zeroize(); self.channel.zeroize(); self.int_code.zeroize(); self.number.zeroize(); self.password.zeroize(); }
}
impl Gateway {
    fn capture(uri: &str, status: i32, registration: GatewayRegistration<'_>, metadata: &[RpcProperty<'_>]) -> Result<Self, RuntimeError> {
        let fields = [uri, registration.authname_prefix, registration.channel_id, registration.delete_int_code,
            registration.sip_number, registration.sip_password];
        if !matches!(status, super::session::STATUS_REGISTER | super::session::STATUS_UNREGISTER)
            || fields.iter().any(|value| value.is_empty() || value.len() > 16_384 || value.chars().any(char::is_control))
            || fields.iter().map(|v| v.len()).sum::<usize>() > MAX_PARAMETER_BYTES { return Err(RuntimeError::InvalidConfiguration); }
        Ok(Self { uri: uri.to_owned(), status, prefix: registration.authname_prefix.to_owned(), channel: registration.channel_id.to_owned(),
            int_code: registration.delete_int_code.to_owned(), number: registration.sip_number.to_owned(), password: registration.sip_password.to_owned(),
            metadata: Property::capture(metadata)? })
    }
    fn registration(&self) -> GatewayRegistration<'_> {
        GatewayRegistration { authname_prefix: &self.prefix, channel_id: &self.channel, delete_int_code: &self.int_code,
            sip_number: &self.number, sip_password: &self.password }
    }
}

enum Operation {
    Login(Login, Vec<Property>), Bootstrap(Zeroizing<Vec<u8>>, Vec<Property>), Configure(u32, Option<Binding>), ArmClientEx(Binding), ConfigureClientEx, ConfigureCalls(CallBinding), Logout(Vec<Property>),
    Swap(Vec<Property>, Option<Gateway>), CompleteBatch(ServerRequestTicket, bool), CompleteDirect(DirectReplyTicket, bool), CompleteCall(CallReplyTicket,bool,Vec<Property>),
    CompleteInvite(super::native_invite_notifications::InviteReplyTicket, bool),
    CompleteIm(Box<(super::im_delivery::ImReplyTicket,Option<super::im_inbox::CommittedIm>)>),
    CompleteImStatus(Box<(super::im_status_delivery::ImStatusTicket,Option<super::im_inbox::CommittedImStatus>)>),
    ImRead(Box<ImReadWork>),
    WriteReceived(Box<ReceivedWork>),
    GetImBinding(oneshot::Sender<Option<Arc<super::im_delivery::ImBinding>>>),
    GetUserEntryAuth(oneshot::Sender<Option<Box<super::arc_session::UserEntryAuth>>>),
    UserMarkRead(Box<UserReadWork>),
    ArmNativeReceivers(std::sync::Arc<super::native_receiver::NativeReceiverSetup>), ConfigureNativeReceivers(u32),
    GetOffline(i64, Vec<Property>), CompleteOfflineHint(super::rpc_offline::IncomingOfflineHint, bool),
    ConfigureCallControl(Vec<Property>),Call(CallControlTicket,CallOperation,Vec<Property>,oneshot::Sender<Result<OwnedCallOperationReply,RuntimeError>>),
}
impl Operation {
    fn byte_length(&self) -> usize {
        match self {
            Self::Login(login, routing) => [&login.identity, &login.domain, &login.selector, &login.phone, &login.key,
                &login.domain_id, &login.application_id, &login.cc].iter().map(|v| v.len()).sum::<usize>()
                + login.properties.iter().chain(routing).map(|p| p.key.len() + p.value.len()).sum::<usize>(),
            Self::Bootstrap(key, routing) => key.len() + routing.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>(),
            Self::Logout(routing) | Self::GetOffline(_, routing) => routing.iter().map(|p| p.key.len() + p.value.len()).sum(),
            Self::Configure(_, incoming) => incoming.as_ref().map_or(0, |b| b.object.len() + b.domain.len() + b.receiver.len()),
            Self::Swap(routing, gateway) => routing.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>()
                + gateway.as_ref().map_or(0, |g| g.uri.len() + g.prefix.len() + g.channel.len() + g.int_code.len() + g.number.len() + g.password.len()
                    + g.metadata.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>()),
            Self::ConfigureClientEx | Self::CompleteBatch(_, _) | Self::CompleteDirect(_, _) | Self::CompleteOfflineHint(_, _) | Self::CompleteInvite(_, _) | Self::CompleteIm(_) | Self::CompleteImStatus(_) => 0,
            Self::ConfigureCalls(binding)=>binding.object.len()+binding.domain.len(),
            Self::ConfigureNativeReceivers(_) => 0,
            Self::ArmNativeReceivers(setup) => setup.retained_bytes(),
            Self::ImRead(work)=>work.query.retained_bytes(),
            Self::WriteReceived(work)=>work.permit.query().retained_bytes(),
            Self::GetImBinding(_)|Self::GetUserEntryAuth(_)=>0,
            Self::UserMarkRead(work)=>work.permit.label().len()+96,
            Self::ArmClientEx(binding)=>binding.object.len()+binding.domain.len()+binding.receiver.len(),
            Self::CompleteCall(_,_,properties)|Self::ConfigureCallControl(properties)|Self::Call(_,_,properties,_)=>properties.iter().map(|p|p.key.len()+p.value.len()).sum(),
        }
    }
}
struct ImReadWork{query:Arc<ImQuery>,gate:Arc<ImReceiptGate>,interest:Arc<ImReadInterest>,reply:oneshot::Sender<Result<OwnedImReply,RuntimeError>>}
struct ImReadPending{query:Arc<ImQuery>,gate:Arc<ImReceiptGate>,interest:Arc<ImReadInterest>,deadline:Instant,reply:oneshot::Sender<Result<OwnedImReply,RuntimeError>>}
struct Command { operation: Operation, completion: oneshot::Sender<Result<RuntimeReceipt, RuntimeError>>, admitted: Instant }
struct CallPending{ticket:CallControlTicket,operation:CallOperation,deadline:Instant,completion:oneshot::Sender<Result<OwnedCallOperationReply,RuntimeError>>}
struct CallRetirement{scope:RpcConnectionScope,ticket:CallControlTicket,done:bool}
impl Drop for CallRetirement{fn drop(&mut self){if !self.done{self.ticket.revoke();self.scope.close();}}}
struct LogoutPending { id: i32, completion: oneshot::Sender<Result<RuntimeReceipt, RuntimeError>> }
struct UnhostingPending { sequence: i32, outer_id: i32, deadline: Instant, completion: oneshot::Sender<Result<RuntimeReceipt, RuntimeError>> }
struct UnhostingRetirement { scope: RpcConnectionScope, completed: bool }
impl Drop for UnhostingRetirement { fn drop(&mut self) { if !self.completed { self.scope.close(); } } }
struct LogoutRetirement(RpcConnectionScope);
impl Drop for LogoutRetirement { fn drop(&mut self) { self.0.close(); } }

pub struct RuntimeReceipt { owner: AttemptOwner, scope: RpcConnectionScope, pub call_id: Option<i32>, direct: Option<super::arc_protocol::DirectReplayFrontier> }
impl RuntimeReceipt {
    fn new(owner: AttemptOwner, scope: RpcConnectionScope, call_id: Option<i32>) -> Self { Self { owner, scope, call_id, direct: None } }
    pub(super) fn direct_replay_frontier(&self) -> Option<&super::arc_protocol::DirectReplayFrontier> { self.direct.as_ref() }
    /// Correlation is not channel readiness. Login/register/poll return only
    /// transport submission; unregister/logout waits also require the original
    /// semantic result. None of these receipts grants a new connection or IO.
    pub fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        self.submitted_on(owner, scope) && self.scope.is_open() && !self.scope.session_revoked()
    }
    /// Historical submission correlation only, never a readiness/open-IO
    /// lease. A fast negative SDK reply may close ARC before its sender's
    /// submission receipt is observed; the terminal callback carries failure.
    pub(super) fn submitted_on(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope)
    }
}
impl fmt::Debug for RuntimeReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeReceipt(<redacted>)") }
}

pub struct RuntimeEvent { owner: AttemptOwner, scope: RpcConnectionScope, events: Vec<OfflineEvent> }
impl RuntimeEvent {
    /// Checked snapshot; close blocks new access. No revoked reference or copied
    /// message is a later channel/readiness lease. Consumer uses original owner.
    pub fn events_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<&[OfflineEvent]> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() && !self.scope.session_revoked() { Some(&self.events) } else { None }
    }
    pub fn into_events(self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<Vec<OfflineEvent>> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() && !self.scope.session_revoked() { Some(self.events) } else { None }
    }
    pub(super) fn terminal_failure(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<super::session::BackendFailure> {
        if &self.owner != owner || !self.scope.matches(scope) { return None; }
        if self.scope.session_revoked() { return Some(super::session::BackendFailure::SessionRevoked); }
        self.events.iter().find_map(|event| match event {
            OfflineEvent::Callback(callback) if &callback.owner == owner => match callback.callback {
                super::controller::Callback::SdkLogin { success: false } => Some(super::session::BackendFailure::AuthenticationRejected),
                super::controller::Callback::HostingAck { success: false } => Some(super::session::BackendFailure::HostingRejected),
                super::controller::Callback::Failure(reason) => Some(reason),
                _ => None,
            },
            _ => None,
        })
    }
    /// Closed ARC failures must retain their ORIGINAL reason. This narrow path
    /// accepts only private actor-produced negative callbacks; it cannot expose
    /// messages, bootstrap data, a positive ACK or channel readiness after close.
    pub(super) fn into_terminal_callback(self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<super::rpc_pending::OwnedRpcCallback> {
        if &self.owner != owner || !self.scope.matches(scope) { return None; }
        self.events.into_iter().find_map(|event| match event {
            OfflineEvent::Callback(callback) if &callback.owner == owner
                && matches!(callback.callback, super::controller::Callback::SdkLogin { success: false }
                    | super::controller::Callback::HostingAck { success: false } | super::controller::Callback::Failure(_)) => Some(callback),
            _ => None,
        })
    }
}
impl fmt::Debug for RuntimeEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeEvent(<redacted>)") }
}

pub struct RuntimeCloseReceipt { io_released: bool, pub reason: RuntimeError }
impl RuntimeCloseReceipt {
    /// True only after the original JoinHandle completed (including unwind/drop
    /// cleanup). Not an assertion that kernel-accepted bytes were recalled.
    pub fn io_released(&self) -> bool { self.io_released }
}
impl fmt::Debug for RuntimeCloseReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeCloseReceipt").field("io_released", &self.io_released).field("reason", &self.reason).finish()
    }
}

/// Cloneable command-only reference to one ORIGINAL actor. It cannot move the
/// actor JoinHandle/IO, create another scope, or claim actual release. Dropping
/// the owning RuntimeHandle still retires all these references.
#[derive(Clone)]
pub struct RuntimeCommands { owner: AttemptOwner, scope: RpcConnectionScope, commands: mpsc::Sender<Command> }
impl fmt::Debug for RuntimeCommands { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeCommands(<redacted>)") } }
impl RuntimeCommands {
    pub fn connection_scope(&self) -> RpcConnectionScope { self.scope.clone() }
    fn accepts(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() && !self.scope.session_revoked()
    }
    fn enqueue(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, operation: Operation)
        -> Result<oneshot::Receiver<Result<RuntimeReceipt, RuntimeError>>, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        if operation.byte_length() > MAX_PARAMETER_BYTES { return Err(RuntimeError::InvalidConfiguration); }
        let (completion, result) = oneshot::channel();
        self.commands.try_send(Command { operation, completion, admitted: Instant::now() }).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => RuntimeError::Backpressure,
            mpsc::error::TrySendError::Closed(_) => RuntimeError::Closed,
        })?;
        Ok(result)
    }
    async fn submit(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, operation: Operation) -> Result<RuntimeReceipt, RuntimeError> {
        self.enqueue(owner, scope, operation)?.await.map_err(|_| RuntimeError::Closed)?
    }
    pub async fn login(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Login(Login::capture(inputs)?, Property::capture(routing)?)).await
    }
    pub async fn bootstrap(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, key: &[u8], routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        if key.is_empty() || key.len() > super::rpc_stream::MAX_FIELD_BYTES || key.iter().any(u8::is_ascii_control) || key.windows(4).any(|v| v == b"ENC(") {
            return Err(RuntimeError::InvalidConfiguration);
        }
        self.submit(owner, scope, Operation::Bootstrap(Zeroizing::new(key.to_vec()), Property::capture(routing)?)).await
    }
    pub async fn configure(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, incarnation: u32, incoming: Option<IncomingBinding<'_>>) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Configure(incarnation, incoming.map(Binding::capture).transpose()?)).await
    }
    pub async fn poll(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Swap(Property::capture(routing)?, None)).await
    }
    pub async fn hosting(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, uri: &str, status: i32, registration: GatewayRegistration<'_>,
        metadata: &[RpcProperty<'_>], routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        if status == super::session::STATUS_UNREGISTER && routing.iter().any(|p| p.key == b"pwd" || p.key == b"token") { return Err(RuntimeError::InvalidConfiguration); }
        let routing = Property::capture(routing)?; let gateway = Gateway::capture(uri, status, registration, metadata)?;
        // No suspension occurs between successful admission and arming this
        // guard. Invalid aggregate inputs or a full queue did not submit any
        // unregister work and must not retire the original live connection.
        let completion = self.enqueue(owner, scope, Operation::Swap(routing, Some(gateway)))?;
        let mut retirement = if status == super::session::STATUS_UNREGISTER { Some(UnhostingRetirement { scope: self.scope.clone(), completed: false }) } else { None };
        let result = completion.await.map_err(|_| RuntimeError::Closed)?;
        if result.is_ok() { if let Some(guard) = &mut retirement { guard.completed = true; } }
        result
    }
    /// 只有账号消费器传入的已提交位置可继续取件；SDK token 仍由原 actor 提供。
    pub(super) async fn get_offline(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, position: i64, routing: &[RpcProperty<'_>])
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || position < -1 || position == i64::MAX { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::GetOffline(position, Property::capture(routing)?)).await
    }
    pub(super) async fn complete_offline_hint(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, hint: &super::rpc_offline::IncomingOfflineHint, accepted: bool)
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !hint.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteOfflineHint(hint.captured_copy(), accepted)).await
    }

    pub async fn complete_batch(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &ServerRequestTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteBatch(ticket.captured_copy(), accepted)).await
    }
    pub async fn complete_direct(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &DirectReplyTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteDirect(ticket.captured_copy(), accepted)).await
    }
    pub(super) async fn complete_im(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&super::im_delivery::ImReplyTicket,proof:Option<&super::im_inbox::CommittedIm>)->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope)||!ticket.matches(owner,scope)||proof.is_some_and(|proof|!proof.authorizes(ticket,Instant::now())){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::CompleteIm(Box::new((ticket.captured_copy(),proof.map(|proof|proof.captured_copy()))))).await
    }
    pub(super) async fn complete_im_status(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&super::im_status_delivery::ImStatusTicket,proof:Option<&super::im_inbox::CommittedImStatus>)->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope)||!ticket.matches(owner,scope)||proof.is_some_and(|proof|!proof.authorizes(ticket,Instant::now())){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::CompleteImStatus(Box::new((ticket.captured_copy(),proof.map(|proof|proof.captured_copy()))))).await
    }
    pub(super) async fn read_im(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,query:Arc<ImQuery>,gate:Arc<ImReceiptGate>)->Result<OwnedImReply,RuntimeError>{
        if !self.accepts(owner,scope)||!gate.live(){return Err(RuntimeError::Scope);}
        if query.operation()==ImOperation::SetReceived{return Err(RuntimeError::InvalidConfiguration);}
        let interest=ImReadInterest::original(scope);let(reply,result)=oneshot::channel();
        let completion=self.enqueue(owner,scope,Operation::ImRead(Box::new(ImReadWork{query,gate:gate.clone(),interest:interest.clone(),reply})))?;
        let _wait=ImReadWait::original(interest);
        completion.await.map_err(|_|RuntimeError::Closed)??;
        let data=result.await.map_err(|_|RuntimeError::Closed)??;
        if !data.matches(owner,scope)||!gate.live(){return Err(RuntimeError::Cancelled);}Ok(data)
    }

    pub(super) async fn complete_invite_notification(&self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        ticket: &super::native_invite_notifications::InviteReplyTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        if accepted && !ticket.positive_is_current() { return Err(RuntimeError::Timeout); }
        self.submit(owner, scope, Operation::CompleteInvite(ticket.captured_copy(), accepted)).await
    }

    pub(super) async fn arm_native_receivers(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, setup: std::sync::Arc<super::native_receiver::NativeReceiverSetup>)
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::ArmNativeReceivers(setup)).await
    }
    pub(super) async fn configure_native_receivers(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, incarnation: u32) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::ConfigureNativeReceivers(incarnation)).await
    }
    pub(super) async fn arm_client_ex(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, binding: IncomingBinding<'_>) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::ArmClientEx(Binding::capture(binding)?)).await
    }
    pub(super) async fn configure_client_ex(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::ConfigureClientEx).await
    }
    pub async fn configure_calls(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,binding:CallIncomingBinding<'_>)->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::ConfigureCalls(CallBinding::capture(binding)?)).await
    }
    pub async fn complete_call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallReplyTicket,accepted:bool,properties:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope)||!ticket.matches(owner,scope){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::CompleteCall(ticket.captured_copy(),accepted,Property::capture(properties)?)).await
    }
    pub async fn configure_call_control(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,routing:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::ConfigureCallControl(logout_properties(routing)?)).await
    }
    pub async fn call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallControlTicket,operation:CallOperation,properties:&[RpcProperty<'_>])->Result<OwnedCallOperationReply,RuntimeError>{
        if !self.accepts(owner,scope)||!ticket.matches(owner,scope)||operation!=CallOperation::Release&&!ticket.is_live(){return Err(RuntimeError::Scope);}
        let properties=Property::capture(properties)?;let(reply,result)=oneshot::channel();
        let completion=self.enqueue(owner,scope,Operation::Call(ticket.captured_copy(),operation,properties,reply))?;
        let mut guard=CallRetirement{scope:scope.clone(),ticket:ticket.captured_copy(),done:false};
        let submitted=completion.await.map_err(|_|RuntimeError::Closed)?;
        if let Err(error)=submitted{if matches!(error,RuntimeError::CallNotReady|RuntimeError::InvalidConfiguration){guard.done=true;}return Err(error);}
        let reply=result.await.map_err(|_|RuntimeError::Closed)?;
        if reply.is_ok()||matches!(&reply,Err(RuntimeError::CallRejected)){guard.done=true;}
        reply
    }
    /// This method returns only the matched original logout RPC result, not an
    /// IO release receipt. Hosting-register returns a submission receipt;
    /// hosting-unregister separately waits for its matched nested result.
    pub async fn logout(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, routing: &[RpcProperty<'_>]) -> Result<(), RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        let routing = logout_properties(routing)?;
        let completion = self.enqueue(owner, scope, Operation::Logout(routing))?;
        let _retirement = LogoutRetirement(self.scope.clone());
        completion.await.map_err(|_| RuntimeError::Closed)?.map(|_| ())
    }
    pub fn request_close(&self) { self.scope.close(); }
}

pub struct RuntimeHandle { owner: AttemptOwner, scope: RpcConnectionScope, commands: mpsc::Sender<Command>, task: Option<JoinHandle<RuntimeError>> }
impl fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeHandle(<redacted>)") }
}
impl RuntimeHandle {
    pub fn command_handle(&self) -> RuntimeCommands { RuntimeCommands { owner: self.owner.clone(), scope: self.scope.clone(), commands: self.commands.clone() } }
    pub fn connection_scope(&self) -> RpcConnectionScope { self.scope.clone() }
    fn accepts(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() && !self.scope.session_revoked()
    }
    async fn submit(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, operation: Operation) -> Result<RuntimeReceipt, RuntimeError> {
        self.command_handle().submit(owner, scope, operation).await
    }
    pub async fn login(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>])
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Login(Login::capture(inputs)?, Property::capture(routing)?)).await
    }
    pub async fn bootstrap(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, application_key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        if application_key.is_empty() || application_key.len() > super::rpc_stream::MAX_FIELD_BYTES
            || application_key.iter().any(u8::is_ascii_control) || application_key.windows(4).any(|v| v == b"ENC(") {
            return Err(RuntimeError::InvalidConfiguration);
        }
        self.submit(owner, scope, Operation::Bootstrap(Zeroizing::new(application_key.to_vec()), Property::capture(routing)?)).await
    }
    pub async fn configure(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, incarnation: u32, incoming: Option<IncomingBinding<'_>>)
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Configure(incarnation, incoming.map(Binding::capture).transpose()?)).await
    }
    pub async fn poll(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Swap(Property::capture(routing)?, None)).await
    }
    pub async fn hosting(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, uri: &str, status: i32,
        registration: GatewayRegistration<'_>, metadata: &[RpcProperty<'_>], routing: &[RpcProperty<'_>]) -> Result<RuntimeReceipt, RuntimeError> {
        self.command_handle().hosting(owner, scope, uri, status, registration, metadata, routing).await
    }
    pub async fn complete_batch(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &ServerRequestTicket, accepted: bool)
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteBatch(ticket.captured_copy(), accepted)).await
    }
    pub async fn complete_direct(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &DirectReplyTicket, accepted: bool)
        -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteDirect(ticket.captured_copy(), accepted)).await
    }
    pub async fn configure_calls(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,binding:CallIncomingBinding<'_>)->Result<RuntimeReceipt,RuntimeError>{
        self.command_handle().configure_calls(owner,scope,binding).await
    }
    pub async fn complete_call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallReplyTicket,accepted:bool,properties:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{
        self.command_handle().complete_call(owner,scope,ticket,accepted,properties).await
    }
    pub async fn configure_call_control(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,routing:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{self.command_handle().configure_call_control(owner,scope,routing).await}
    pub async fn call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallControlTicket,operation:CallOperation,properties:&[RpcProperty<'_>])->Result<OwnedCallOperationReply,RuntimeError>{self.command_handle().call(owner,scope,ticket,operation,properties).await}
    /// Confirm native logout separately from actual original IO/task release.
    /// Wrong-owner requests have no close side effect. Failed/cancelled logout
    /// still closes original IO; a cancelled join can be waited again via close.
    pub async fn logout(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, routing: &[RpcProperty<'_>]) -> Result<RuntimeCloseReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        logout_properties(routing)?;
        let outcome = self.command_handle().logout(owner, scope, routing).await;
        let receipt = self.close().await;
        outcome?;
        if receipt.io_released && receipt.reason == RuntimeError::LoggedOut { Ok(receipt) } else { Err(receipt.reason) }
    }
    pub fn request_close(&self) { self.scope.close(); }
    /// Confirm only after the original actor released its IO, TLS, queues and
    /// ARC pending work. Caller cancellation still requests terminal close.
    pub async fn close(&mut self) -> RuntimeCloseReceipt {
        self.request_close();
        let (io_released, reason) = match self.task.as_mut() {
            Some(task) => (true, task.await.unwrap_or(RuntimeError::Cancelled)),
            None => (false, RuntimeError::Closed),
        };
        self.task = None; RuntimeCloseReceipt { io_released, reason }
    }
}
impl Drop for RuntimeHandle { fn drop(&mut self) { self.scope.close(); } }

/// Prepare original ARC/TLS ownership BEFORE DNS/connect. This scope is the
/// same one later attached to the IO actor; cancellation cannot mint a new
/// actor scope or leave a detached connection/login waiting in the background.
pub struct PreparedRuntime {
    owner: AttemptOwner, scope: RpcConnectionScope, backend: Option<Box<OfflineBackend>>, tls: Option<TlsEngine>,
    policy: Option<RuntimePolicy>, timing: Option<RouterTiming>, start: Instant, attached: bool,
}
impl fmt::Debug for PreparedRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PreparedRuntime(<redacted>)") }
}
impl Drop for PreparedRuntime {
    fn drop(&mut self) { if !self.attached { self.scope.close(); } }
}
impl PreparedRuntime {
    pub fn connection_scope(&self) -> RpcConnectionScope { self.scope.clone() }
    /// Consumes exactly the original prepared scope. All original TLS deadlines
    /// include connection setup time; attach does not reset the clock or trust.
    pub fn attach<I>(mut self, io: I) -> Result<(RuntimeHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>
        where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
        if !self.scope.is_open() { return Err(RuntimeError::Scope); }
        let mut tls = self.tls.take().ok_or(RuntimeError::Closed)?;
        if !tls.tick(&self.owner, &self.scope, Instant::now())? { return Err(RuntimeError::Closed); }
        let backend = self.backend.take().ok_or(RuntimeError::Closed)?;
        let policy = self.policy.take().ok_or(RuntimeError::Closed)?;
        let (commands_tx, commands_rx) = mpsc::channel(MAX_COMMANDS); let (events_tx, events_rx) = mpsc::channel(MAX_EVENTS);
        let owner = self.owner.clone(); let scope = self.scope.clone();
        let actor = Actor { io: Some(io), owner: owner.clone(), scope: scope.clone(), backend, tls:Some(tls), common:None, native_attempt_end:None, commands: commands_rx,
            events: events_tx, policy, timing: self.timing, start: self.start, pending: std::collections::BTreeMap::new(),
            logout_pending: None, unhosting_pending: None,call_pending:std::collections::BTreeMap::new(),im_pending:std::collections::BTreeMap::new(),received_pending:std::collections::BTreeMap::new(),user_read_pending:std::collections::BTreeMap::new(),user_write_retirement:None,received_write_retirement:None };
        self.attached = true;
        let task = tokio::spawn(actor.run());
        Ok((RuntimeHandle { owner, scope, commands: commands_tx, task: Some(task) }, events_rx))
    }
}

pub fn prepare(owner: AttemptOwner, route: PacketRoute<'_>, tls_parameters: TlsParameters<'_>,
    policy: RuntimePolicy, timing: Option<RouterTiming>) -> Result<PreparedRuntime, RuntimeError> {
    if !policy.valid() { return Err(RuntimeError::InvalidConfiguration); }
    let start = Instant::now(); let mut backend = Box::new(OfflineBackend::new(owner.clone())); let scope = backend.connection_scope();
    let tls = backend.take_tls_engine(route, tls_parameters, start)?;
    Ok(PreparedRuntime { owner, scope, backend: Some(backend), tls: Some(tls), policy: Some(policy), timing, start, attached: false })
}

/// Start only on caller-provided nonblocking IO, never a guessed network target.
/// This low-level runtime is not installed as a ready HostingBackend/provider.
pub fn start<I>(io: I, owner: AttemptOwner, route: PacketRoute<'_>, tls_parameters: TlsParameters<'_>,
    policy: RuntimePolicy, timing: Option<RouterTiming>) -> Result<(RuntimeHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>
    where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    prepare(owner, route, tls_parameters, policy, timing)?.attach(io)
}

struct Actor<I> {
    // Retain the original engine allocation across prepared/connected/actor
    // handoff; boxing does not spawn an owner, reauthenticate or change IO.
    io: Option<I>, owner: AttemptOwner, scope: RpcConnectionScope, backend: Box<OfflineBackend>, tls: Option<TlsEngine>,
    common: Option<super::common_runtime::CommonPath>, native_attempt_end:Option<Instant>,
    commands: mpsc::Receiver<Command>, events: mpsc::Sender<RuntimeEvent>, policy: RuntimePolicy,
    timing: Option<RouterTiming>, start: Instant, pending: std::collections::BTreeMap<i32, Instant>, logout_pending: Option<LogoutPending>, unhosting_pending: Option<UnhostingPending>,
    call_pending:std::collections::BTreeMap<i32,CallPending>,
    im_pending:std::collections::BTreeMap<i32,ImReadPending>,
    received_pending:std::collections::BTreeMap<i32,ReceivedPending>,
    user_read_pending:std::collections::BTreeMap<i32,UserReadPending>,
    user_write_retirement:Option<RuntimeError>,
    received_write_retirement:Option<RuntimeError>,
}
impl<I> Drop for Actor<I> {
    fn drop(&mut self) {
        self.scope.close(); self.backend.close(); if let Some(tls)=&mut self.tls{tls.close();}
        if let Some(common)=&mut self.common{common.close();}self.common=None;self.io = None; self.commands.close();
    }
}
impl<I> Actor<I> where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    async fn run(mut self) -> RuntimeError {
        let result = self.loop_run().await;
        let result = if result==RuntimeError::Closed{self.user_write_close_reason().unwrap_or(result)}else{result};
        let result = if result==RuntimeError::Closed{self.received_write_close_reason().unwrap_or(result)}else{result};
        let result = if self.scope.session_revoked() { RuntimeError::SessionRevoked } else { result };
        self.scope.close(); self.backend.close(); if let Some(tls)=&mut self.tls{tls.close();}
        if let Some(common)=&mut self.common{common.close();}self.common=None;self.io = None; self.commands.close();
        while let Ok(command) = self.commands.try_recv() { let _ = command.completion.send(Err(RuntimeError::Closed)); }
        result
    }
    fn retire_im_reads(&mut self,now:Instant){
        self.retire_received_writes(now);
        self.retire_user_read_writes(now);
        let retired=self.im_pending.iter().filter_map(|(&id,p)|{
            if p.interest.is_retired()||p.reply.is_closed()||!p.gate.live(){Some((id,RuntimeError::Cancelled))}
            else if now>=p.deadline{Some((id,RuntimeError::Timeout))}else{None}
        }).collect::<Vec<_>>();
        for(id,reason)in retired{if let Some(p)=self.im_pending.remove(&id){
            self.backend.retire_im_read(id);self.pending.remove(&id);p.interest.cancel();let _=p.reply.send(Err(reason));
        }}
    }
    fn elapsed_ms(&self, now: Instant) -> u64 { now.saturating_duration_since(self.start).as_millis().min(u64::MAX as u128) as u64 }
    async fn loop_run(&mut self) -> RuntimeError {
        if self.common.is_some(){return self.common_loop_run().await;}
        let initial = match self.tls.as_mut().expect("original TLS mode").take_wire_output(&self.owner, &self.scope, self.timing, Instant::now()) {
            Ok(initial) => initial, Err(error) => return error.into(),
        };
        if let Err(error) = self.send_wire(&initial, None).await { return error; }
        let mut read = [0; 8192]; let mut timer = tokio::time::interval(Duration::from_millis(20));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.scope.session_revoked() { return RuntimeError::SessionRevoked; }
            if !self.scope.is_open() { return RuntimeError::Closed; }
            tokio::select! {
                biased;
                _ = self.scope.closed() => return RuntimeError::Closed,
                _ = self.events.closed() => return RuntimeError::Closed,
                _ = timer.tick() => {
                    let now = Instant::now();
                    self.retire_im_reads(now);
                    if let Err(error) = self.tls.as_mut().expect("original TLS mode").tick(&self.owner, &self.scope, now) { return error.into(); }
                    self.pending.retain(|id, _| self.backend.is_pending(*id));
                    if self.logout_pending.as_ref().is_some_and(|pending| pending.completion.is_closed()) { return RuntimeError::Cancelled; }
                    if let Some(pending) = &self.unhosting_pending {
                        if pending.completion.is_closed() { return RuntimeError::Cancelled; }
                        if now >= pending.deadline { return RuntimeError::Timeout; }
                    }
                    if self.call_pending.values().any(|pending|pending.completion.is_closed()||now>=pending.deadline){return RuntimeError::Timeout;}
                    if self.pending.values().any(|deadline| now >= *deadline) { return RuntimeError::Timeout; }
                }
                received = self.io.as_mut().expect("owned IO").read(&mut read) => {
                    let count = match received { Ok(0) => { let _ = self.tls.as_mut().expect("original TLS mode").finish_eof(&self.owner, &self.scope); return RuntimeError::Io; },
                        Ok(count) => count, Err(_) => return RuntimeError::Io };
                    let now = Instant::now();
                    let progress = match self.tls.as_mut().expect("original TLS mode").on_wire_bytes(&self.owner, &self.scope, &read[..count], self.timing, now) {
                        Ok(Some(progress)) => progress, Ok(None) => return RuntimeError::Closed, Err(error) => return error.into(),
                    };
                    if let Err(error) = self.send_wire(&progress.wire, None).await { return error; }
                    let elapsed_ms = self.elapsed_ms(now);
                    self.retire_im_reads(now);
                    let events = match self.backend.on_verified_plaintext(&self.owner, &self.scope, progress.plaintext, elapsed_ms) {
                        Ok(events) => events, Err(_) => return RuntimeError::Protocol,
                    };
                    self.pending.retain(|id, _| self.backend.is_pending(*id));
                    if let Err(error) = self.deliver(events).await { return error; }
                    if self.backend.phase() == ArcPhase::Closed { return RuntimeError::Protocol; }
                }
                command = self.commands.recv() => {
                    let Some(command) = command else { return RuntimeError::Closed; };
                    if command.completion.is_closed() { continue; }
                    if let Err(error) = self.command(command).await { return error; }
                }
            }
        }
    }
    async fn deliver(&mut self, events: Vec<OfflineEvent>) -> Result<(), RuntimeError> {
        let mut unhosting_accepted = false;let mut call_replies=Vec::new();
        let outcome = self.deliver_events(events, &mut unhosting_accepted,&mut call_replies).await;
        for reply in call_replies{
            let pending=self.call_pending.remove(&reply.request_id()).ok_or(RuntimeError::Protocol)?;
            if !reply.matches(&self.owner,&self.scope)||reply.native_id()!=pending.ticket.native_id()||reply.operation()!=pending.operation{return Err(RuntimeError::Protocol);}
            if Instant::now()>=pending.deadline||!self.scope.is_open(){let _=pending.completion.send(Err(RuntimeError::Timeout));return Err(RuntimeError::Timeout);}
            let result=outcome.and_then(|()|if reply.accepted(){Ok(reply)}else{Err(RuntimeError::CallRejected)});
            if pending.completion.send(result).is_err(){return Err(RuntimeError::Cancelled);}
        }
        for reply in self.backend.take_im_replies(){
            let id=reply.request().call_id();let Some(pending)=self.im_pending.remove(&id)else{continue;};self.pending.remove(&id);
            if pending.interest.is_retired()||pending.reply.is_closed()||!pending.gate.live(){pending.interest.cancel();let _=pending.reply.send(Err(RuntimeError::Cancelled));continue;}
            if Instant::now()>=pending.deadline{pending.interest.cancel();let _=pending.reply.send(Err(RuntimeError::Timeout));continue;}
            if !reply.matches(&self.owner,&self.scope)||!Arc::ptr_eq(&pending.gate,&reply.request().gate())||!Arc::ptr_eq(&pending.query,reply.request().query_arc()){
                let _=pending.reply.send(Err(RuntimeError::Protocol));return Err(RuntimeError::Protocol);
            }
            let result=outcome.map(|()|reply);pending.interest.cancel();let _=pending.reply.send(result);
        }
        self.deliver_received_replies(outcome)?;
        self.deliver_user_read_replies(outcome)?;
        // Keep the original wait (and its deadline) installed while every
        // outgoing reply and visible event from this delivery can still fail.
        // A semantic true followed by an IO/queue error is not an open-IO
        // receipt on which the caller may proceed to SDK logout.
        if unhosting_accepted || outcome.is_err() {
            if let Some(pending) = self.unhosting_pending.take() {
                let result = outcome.and_then(|()| {
                    if Instant::now() >= pending.deadline { return Err(RuntimeError::Timeout); }
                    if !self.scope.is_open() { return Err(RuntimeError::Closed); }
                    Ok(RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), Some(pending.outer_id)))
                });
                let error = result.as_ref().err().copied();
                if pending.completion.send(result).is_err() { return Err(RuntimeError::Cancelled); }
                if let Some(error) = error { return Err(error); }
            }
        }
        outcome
    }
    async fn deliver_events(&mut self, events: Vec<OfflineEvent>, unhosting_accepted: &mut bool,call_replies:&mut Vec<OwnedCallOperationReply>) -> Result<(), RuntimeError> {
        let mut visible = Vec::new();
        let mut revoked = false;
        for event in events {
            match event {
                OfflineEvent::ClientEx(notice) => {
                    if notice.is_revocation_for(&self.owner, &self.scope) {
                        // No positive message/ACK/channel survives a revoked
                        // session in this group. The sticky scope preserves the
                        // reason even when this terminal queue is already full.
                        visible.clear();
                        visible.push(OfflineEvent::Callback(super::rpc_pending::OwnedRpcCallback {
                            owner: self.owner.clone(), callback: super::controller::Callback::Failure(super::session::BackendFailure::SessionRevoked),
                        }));
                        revoked = true; break;
                    }
                    if !notice.matches(&self.owner, &self.scope) { return Err(RuntimeError::Scope); }
                    visible.push(OfflineEvent::ClientEx(notice));
                }
                OfflineEvent::CallOperation(reply)=>{
                    if !self.call_pending.contains_key(&reply.request_id()){return Err(RuntimeError::Protocol);}
                    call_replies.push(reply);
                }
                OfflineEvent::Unhosting(reply) => {
                    let pending = self.unhosting_pending.as_ref().ok_or(RuntimeError::Protocol)?;
                    // A result may arrive between timer ticks. A repeated or
                    // expired result cannot turn this original wait into Ok.
                    if *unhosting_accepted { return Err(RuntimeError::Protocol); }
                    if Instant::now() >= pending.deadline { return Err(RuntimeError::Timeout); }
                    if !reply.matches(&self.owner, &self.scope) || reply.sequence() != pending.sequence { return Err(RuntimeError::Protocol); }
                    let accepted = reply.matches(&self.owner, &self.scope) && reply.sequence() == pending.sequence && reply.accepted();
                    if !accepted { return Err(if reply.rejected() { RuntimeError::UnregistrationRejected } else { RuntimeError::Protocol }); }
                    *unhosting_accepted = true;
                }
                OfflineEvent::Logout(reply) => {
                    let pending = self.logout_pending.take().ok_or(RuntimeError::Protocol)?;
                    let accepted = reply.matches(&self.owner, &self.scope) && reply.call_id() == pending.id && reply.accepted();
                    let result = if accepted { Ok(RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), Some(pending.id))) }
                        else { Err(RuntimeError::Protocol) };
                    if pending.completion.send(result).is_err() { return Err(RuntimeError::Cancelled); }
                    return Err(if accepted { RuntimeError::LoggedOut } else { RuntimeError::Protocol });
                }
                OfflineEvent::Outgoing(packets) => { self.send_packets(&packets.packets,None,None).await?; }
                event => visible.push(event),
            }
        }
        if !visible.is_empty() {
            self.events.try_send(RuntimeEvent { owner: self.owner.clone(), scope: self.scope.clone(), events: visible })
                .map_err(|_| RuntimeError::Backpressure)?;
        }
        if revoked { Err(RuntimeError::SessionRevoked) } else { Ok(()) }
    }
    async fn command(&mut self, mut command: Command) -> Result<(), RuntimeError> {
        if self.scope.session_revoked() { let _ = command.completion.send(Err(RuntimeError::SessionRevoked)); return Err(RuntimeError::SessionRevoked); }
        if !self.scope.is_open() { let _ = command.completion.send(Err(RuntimeError::Closed)); return Err(RuntimeError::Closed); }
        self.retire_im_reads(Instant::now());
        let now = Instant::now();
        if self.unhosting_pending.as_ref().is_some_and(|pending| now >= pending.deadline) {
            let _ = command.completion.send(Err(RuntimeError::Timeout));
            return Err(RuntimeError::Timeout);
        }
        if self.unhosting_pending.is_some() && (matches!(&command.operation, Operation::CompleteInvite(_, true))||matches!(&command.operation,Operation::CompleteIm(data) if data.1.is_some())) {
            let _ = command.completion.send(Err(RuntimeError::Closed)); return Ok(());
        }
        if self.unhosting_pending.is_some() && matches!(&command.operation,Operation::CompleteImStatus(data) if data.1.is_some()){
            let _=command.completion.send(Err(RuntimeError::Closed));return Ok(());
        }
        if self.logout_pending.is_some() || self.unhosting_pending.is_some() && !matches!(&command.operation, Operation::Swap(_, None) | Operation::CompleteBatch(_, _) | Operation::CompleteDirect(_, _) | Operation::CompleteCall(_,_,_) | Operation::CompleteOfflineHint(_, _) | Operation::CompleteInvite(_, _) | Operation::CompleteIm(_) | Operation::CompleteImStatus(_)) {
            let _ = command.completion.send(Err(RuntimeError::Closed)); return Ok(());
        }
        if !matches!(&command.operation, Operation::ArmClientEx(_) | Operation::ArmNativeReceivers(_)) && self.common.is_none() && self.tls.as_ref().is_none_or(|tls|tls.phase()!=TlsPhase::Verified) {
            let _ = command.completion.send(Err(RuntimeError::Tls(TlsError::NotReady))); return Ok(());
        }
        let mut invite_claims = match &command.operation { Operation::CompleteInvite(ticket, true) => {
            if !ticket.positive_is_current() { let _ = command.completion.send(Err(RuntimeError::Timeout)); return Ok(()); }
            ticket.consumer_claim().into_iter().collect::<Vec<_>>()
        }, _ => Vec::new() };
        let mut invite_deadline = match &command.operation { Operation::CompleteInvite(ticket, true) => Some(ticket.completion_deadline()), _ => None };
        let mut im_gates=Vec::new();
        if let Operation::CompleteIm(data)=&command.operation{if let Some(proof)=&data.1{
            if !proof.authorizes(&data.0,now){let _=command.completion.send(Err(RuntimeError::Scope));return Ok(());}
            im_gates.push(proof.gate());let end=data.0.completion_deadline();invite_deadline=Some(invite_deadline.map_or(end,|original|original.min(end)));
        }}
        if let Operation::CompleteImStatus(data)=&command.operation{if let Some(proof)=&data.1{
            if !proof.authorizes(&data.0,now){let _=command.completion.send(Err(RuntimeError::Scope));return Ok(());}
            im_gates.push(proof.gate());let end=data.0.deadline();invite_deadline=Some(invite_deadline.map_or(end,|original|original.min(end)));
        }}
        if matches!(&command.operation, Operation::Swap(_, _)) {
            let (gates,deadline)=self.backend.im_write_guard();im_gates.extend(gates);
            if let Some(deadline)=deadline{invite_deadline=Some(invite_deadline.map_or(deadline,|original|original.min(deadline)));}
            let (claims, deadline) = self.backend.invite_write_guard(); invite_claims.extend(claims);
            if let Some(deadline) = deadline { invite_deadline = Some(invite_deadline.map_or(deadline, |original| original.min(deadline))); }
        }
        let native_deadline=if self.common.is_some(){
            let wait=if matches!(&command.operation,Operation::Login(..)){Duration::from_secs(90)}else{self.policy.rpc_timeout};
            let mut end=command.admitted.checked_add(wait).ok_or(RuntimeError::Timeout)?;
            if self.backend.phase()!=ArcPhase::Authenticated{if let Some(attempt)=self.native_attempt_end{end=end.min(attempt);}}
            if now>=end{let _=command.completion.send(Err(RuntimeError::Timeout));return Err(RuntimeError::Timeout);}Some(end)
        }else{None};
        let native_deadline = match (native_deadline, invite_deadline) {
            (Some(original), Some(invite)) => Some(original.min(invite)), (None, Some(invite)) => Some(invite), (original, None) => original,
        };
        if native_deadline.is_some_and(|deadline| now >= deadline) { let _ = command.completion.send(Err(RuntimeError::Timeout)); return Ok(()); }
        if self.pending.len() >= MAX_PENDING_RPC && matches!(&command.operation, Operation::Login(_, _) | Operation::Bootstrap(_, _) | Operation::Swap(_, _) | Operation::Call(_,_,_,_) | Operation::GetOffline(_, _) | Operation::ImRead(_) | Operation::WriteReceived(_) | Operation::UserMarkRead(_)) {
            let _ = command.completion.send(Err(RuntimeError::Backpressure)); return Ok(());
        }
        let is_response = matches!(&command.operation, Operation::CompleteDirect(_, _) | Operation::CompleteCall(_,_,_) | Operation::CompleteOfflineHint(_, _) | Operation::CompleteInvite(_, _) | Operation::CompleteIm(_) | Operation::CompleteImStatus(_));
        let is_logout = matches!(&command.operation, Operation::Logout(_));
        let mut unhosting_sequence = None;let mut call_wait=None;let mut im_wait=None;let mut received_wait=None;let mut user_wait=None;
        let elapsed_ms = self.elapsed_ms(now);
        let prepared = match command.operation {
            Operation::Configure(epoch, incoming) => {
                let result = self.backend.configure(epoch, incoming.as_ref().map(Binding::borrowed)).map_err(|_| RuntimeError::Protocol);
                let receipt = result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None));
                if command.completion.send(receipt).is_err() { return Err(RuntimeError::Cancelled); }
                return Ok(());
            }
            Operation::ArmNativeReceivers(setup) => {
                let result = self.backend.arm_native_receivers(setup).map_err(|_| RuntimeError::InvalidConfiguration);
                if command.completion.send(result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::ConfigureNativeReceivers(incarnation) => {
                let result = self.backend.configure_native_receivers(incarnation).map_err(|_| RuntimeError::InvalidConfiguration);
                if command.completion.send(result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::ArmClientEx(binding) => {
                let result = self.backend.arm_client_ex(binding.borrowed()).map_err(|_| RuntimeError::InvalidConfiguration);
                if command.completion.send(result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::ConfigureClientEx => {
                let result = self.backend.configure_client_ex().map_err(|_| RuntimeError::InvalidConfiguration);
                if command.completion.send(result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::ConfigureCallControl(routing)=>{
                let result=self.backend.configure_call_control(&borrowed(&routing)).map_err(|_|RuntimeError::InvalidConfiguration);
                if command.completion.send(result.map(|()|RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None))).is_err(){return Err(RuntimeError::Cancelled);}
                return Ok(());
            }
            Operation::Call(ticket,operation,properties,reply)=>{
                if operation==CallOperation::Inform&&!self.backend.call_acknowledgement_ready(&ticket){
                    let _=reply.send(Err(RuntimeError::CallNotReady));let _=command.completion.send(Err(RuntimeError::CallNotReady));return Ok(());
                }
                match self.backend.prepare_call(&ticket,operation,&borrowed(&properties)){
                    Ok(packets)=>{call_wait=Some((ticket,operation,reply));Ok(packets)}
                    Err(_)=>{let _=reply.send(Err(RuntimeError::InvalidConfiguration));let _=command.completion.send(Err(RuntimeError::InvalidConfiguration));return Ok(());}
                }
            }
            Operation::ImRead(work)=>{
                if self.common.is_none()||work.query.operation()==ImOperation::SetReceived{
                    let _=work.reply.send(Err(RuntimeError::InvalidConfiguration));let _=command.completion.send(Err(RuntimeError::InvalidConfiguration));return Ok(());
                }
                if !work.gate.live()||work.interest.is_retired(){
                    work.interest.cancel();let _=work.reply.send(Err(RuntimeError::Cancelled));let _=command.completion.send(Err(RuntimeError::Cancelled));return Ok(());
                }
                match self.backend.prepare_im_read(work.query.clone(),work.gate.clone(),native_deadline.ok_or(RuntimeError::Timeout)?){
                    Ok(packets)=>{im_wait=Some(*work);Ok(packets)}
                    Err(_)=>{let _=work.reply.send(Err(RuntimeError::InvalidConfiguration));let _=command.completion.send(Err(RuntimeError::InvalidConfiguration));return Ok(());}
                }
            }
            Operation::WriteReceived(work)=>{
                if self.common.is_none()||!work.permit.matches(&self.owner,&self.scope)||work.reply.is_closed(){
                    work.permit.cancel();let _=work.reply.send(Err(RuntimeError::Scope));let _=command.completion.send(Err(RuntimeError::Scope));return Ok(());
                }
                match self.backend.prepare_received_write(work.permit.clone(),native_deadline.ok_or(RuntimeError::Timeout)?){
                    Ok(packets)=>{received_wait=Some(*work);Ok(packets)}
                    Err(_)=>{work.permit.cancel();let _=work.reply.send(Err(RuntimeError::InvalidConfiguration));let _=command.completion.send(Err(RuntimeError::InvalidConfiguration));return Ok(());}
                }
            }
            Operation::GetUserEntryAuth(reply)=>{
                let auth=if self.common.is_some(){self.backend.captured_user_entry_auth().ok()}else{None};
                let _=reply.send(auth);let _=command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None)));return Ok(());
            }
            Operation::GetImBinding(reply)=>{
                let binding=if self.common.is_some(){self.backend.captured_im_binding()}else{None};
                let _=reply.send(binding);let _=command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None)));return Ok(());
            }
            Operation::UserMarkRead(work)=>{
                let deadline=native_deadline.ok_or(RuntimeError::Timeout)?.min(work.permit.deadline());
                if self.common.is_none()||!work.permit.matches(&self.owner,&self.scope)||work.reply.is_closed()||now>=deadline{work.permit.cancel();let _=work.reply.send(Err(RuntimeError::Scope));let _=command.completion.send(Err(RuntimeError::Scope));return Ok(());}
                match self.backend.prepare_user_read_write(work.permit.clone(),deadline){Ok(p)=>{user_wait=Some(*work);Ok(p)},Err(_)=>{work.permit.cancel();let _=work.reply.send(Err(RuntimeError::InvalidConfiguration));let _=command.completion.send(Err(RuntimeError::InvalidConfiguration));return Ok(());}}
            }
            Operation::ConfigureCalls(binding)=>{
                let result=self.backend.configure_calls(binding.borrowed()).map_err(|_|RuntimeError::Protocol);
                if command.completion.send(result.map(|()|RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None))).is_err(){return Err(RuntimeError::Cancelled);}
                return Ok(());
            }
            Operation::CompleteCall(ticket,accepted,properties)=>{
                match self.backend.complete_call(&ticket,accepted,&borrowed(&properties)){
                    Ok(Some(super::offline_backend::CallCompletion::Direct(packets)))=>Ok(packets),
                    Ok(Some(super::offline_backend::CallCompletion::BatchQueued))=>{
                        if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None))).is_err(){return Err(RuntimeError::Cancelled);}
                        return Ok(());
                    }
                    _=>{let _=command.completion.send(Err(RuntimeError::Protocol));return Ok(());}
                }
            }
            Operation::CompleteIm(data)=>{
                match self.backend.complete_im(&data.0,data.1.as_ref()){
                    Ok(Some(super::offline_backend::CallCompletion::Direct(packets)))=>Ok(packets),
                    Ok(Some(super::offline_backend::CallCompletion::BatchQueued))=>{
                        if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None))).is_err(){return Err(RuntimeError::Cancelled);}return Ok(());
                    }
                    _=>{let _=command.completion.send(Err(RuntimeError::Protocol));return Ok(());}
                }
            }
            Operation::CompleteImStatus(data)=>{
                match self.backend.complete_im_status(&data.0,data.1.as_ref()){
                    Ok(Some(super::offline_backend::CallCompletion::Direct(packets)))=>Ok(packets),
                    Ok(Some(super::offline_backend::CallCompletion::BatchQueued))=>{
                        if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),None))).is_err(){return Err(RuntimeError::Cancelled);}return Ok(());
                    }
                    _=>{let _=command.completion.send(Err(RuntimeError::Protocol));return Ok(());}
                }
            }
            Operation::CompleteInvite(ticket, accepted) => {
                match self.backend.complete_invite_notification(&ticket, accepted) {
                    Ok(Some(super::offline_backend::CallCompletion::Direct(packets))) => Ok(packets),
                    Ok(Some(super::offline_backend::CallCompletion::BatchQueued)) => {
                        if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() { return Err(RuntimeError::Cancelled); }
                        return Ok(());
                    }
                    _ => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
                }
            }
            Operation::CompleteBatch(ticket, accepted) => {
                let result = self.backend.complete_message(&ticket, accepted).and_then(|completed| {
                    if completed { Ok(()) } else { Err(super::arc_protocol::ProtocolError::Rpc(super::rpc_stream::CodecError::InvalidField)) }
                }).map_err(|_| RuntimeError::Protocol);
                if command.completion.send(result.map(|()| RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::CompleteDirect(ticket, accepted) => match self.backend.complete_direct_message(&ticket, accepted) {
                Ok(Some(packets)) => Ok(packets),
                _ => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
            },
            Operation::GetOffline(position, routing) => self.backend.prepare_offline(position, &borrowed(&routing)),
            Operation::CompleteOfflineHint(hint, accepted) => {
                match self.backend.complete_offline_hint(&hint, accepted) {
                    Ok(Some(super::offline_backend::CallCompletion::Direct(packets))) => Ok(packets),
                    Ok(Some(super::offline_backend::CallCompletion::BatchQueued)) => {
                        if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), None))).is_err() { return Err(RuntimeError::Cancelled); }
                        return Ok(());
                    }
                    _ => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
                }
            }
            Operation::Logout(routing) => self.backend.prepare_logout(&borrowed(&routing)),
            Operation::Bootstrap(key, routing) => self.backend.prepare_bootstrap(&key, &borrowed(&routing)),
            Operation::Login(login, routing) => {
                let properties = borrowed(&login.properties); let routing = borrowed(&routing);
                self.backend.prepare_login(login.inputs(&properties), &routing)
            }
            Operation::Swap(routing, gateway) => {
                let routing = borrowed(&routing);
                if let Some(gateway) = gateway {
                    let metadata = borrowed(&gateway.metadata);
                    let sequence = match self.backend.queue_hosting(self.owner.clone(), &gateway.uri, gateway.status, &gateway.registration(), &metadata) {
                        Ok(sequence) => sequence, Err(_) => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
                    };
                    if gateway.status == super::session::STATUS_UNREGISTER { unhosting_sequence = Some(sequence); }
                    match self.backend.prepare_swap(elapsed_ms, &routing) {
                        Ok(packets) => Ok(packets), Err(error) => { self.backend.retire_hosting(sequence); Err(error) }
                    }
                } else { self.backend.prepare_swap(elapsed_ms, &routing) }
            }
        };
        let packets = match prepared {
            Ok(packets) => packets,
            Err(_) => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
        };
        if let Some(work)=&im_wait{
            if !work.interest.begin_write(&work.gate){self.backend.retire_im_read(packets.call_id);let _=command.completion.send(Err(RuntimeError::Cancelled));return Ok(());}
            im_gates.push(work.gate.clone());
        }
        let read_interest=im_wait.as_ref().map(|work|work.interest.clone());
        if let Some(work)=&received_wait{
            if !work.permit.begin_write(packets.call_id){work.permit.cancel();self.backend.retire_received_write(packets.call_id);let _=command.completion.send(Err(RuntimeError::Cancelled));return Ok(());}
            im_gates.push(work.permit.gate());
        }
        let received_interest=received_wait.as_ref().map(|work|work.permit.clone());
        if let Some(work)=&user_wait{
            if !work.permit.begin_write(packets.call_id){work.permit.cancel();self.backend.retire_user_read_write(packets.call_id);let _=command.completion.send(Err(RuntimeError::Cancelled));return Ok(());}
            im_gates.push(work.permit.gate());im_gates.push(work.permit.parent_gate());
        }
        let user_interest=user_wait.as_ref().map(|work|work.permit.clone());
        let native_deadline=if let Some(work)=&received_wait{Some(work.permit.effective_deadline(native_deadline.ok_or(RuntimeError::Timeout)?))}else if let Some(work)=&user_wait{Some(native_deadline.ok_or(RuntimeError::Timeout)?.min(work.permit.deadline()))}else{native_deadline};
        if is_logout { self.pending.retain(|id, _| self.backend.is_pending(*id)); }
        // Keep one heap-owned transport future, not two large inline copies
        // in the new guarded/legacy branches. Standard test stack and all IO
        // deadlines/gates remain unchanged, including when Invite is absent.
        let mut sending = Box::pin(self.send_packets(&packets.packets,Some(&mut command.completion),native_deadline));
        let sent = if !invite_claims.is_empty() || !im_gates.is_empty() {
            let cancelled = async {
                use futures_util::StreamExt;
                let mut waiting = futures_util::stream::FuturesUnordered::new();
                let mut im_waiting=futures_util::stream::FuturesUnordered::new();
                for claim in &invite_claims { waiting.push(claim.cancelled()); }
                for gate in &im_gates { im_waiting.push(gate.retired()); }
                tokio::select!{biased;
                    _=waiting.next(),if !waiting.is_empty()=>{},_=im_waiting.next(),if !im_waiting.is_empty()=>{},
                    _=async{if let Some(interest)=&read_interest{interest.retired().await;}else{std::future::pending::<()>().await;}},if read_interest.is_some()=>{},
                    _=async{if let Some(permit)=&received_interest{permit.cancelled().await;}else{std::future::pending::<()>().await;}},if received_interest.is_some()=>{},
                    _=async{if let Some(permit)=&user_interest{permit.cancelled().await;}else{std::future::pending::<()>().await;}},if user_interest.is_some()=>{},
                }
            };
            tokio::select! { biased;
                _ = cancelled => Err(RuntimeError::Cancelled),
                result = sending.as_mut() => result,
            }
        } else { sending.as_mut().await };
        drop(sending);
        if let Err(error) = sent {
            if let Some(work)=&received_wait{work.permit.cancel();}
            if let Some(work)=&user_wait{work.permit.cancel();}
            self.backend.retire_request(packets.call_id); let _ = command.completion.send(Err(error)); return Err(error);
        }
        let rpc_deadline=native_deadline.unwrap_or(Instant::now().checked_add(self.policy.rpc_timeout).ok_or(RuntimeError::Timeout)?);
        if let Some(work)=im_wait{
            if !work.interest.submitted(){self.backend.retire_im_read(packets.call_id);let _=command.completion.send(Err(RuntimeError::Cancelled));return Err(RuntimeError::Cancelled);}
            if !work.gate.live()||work.interest.is_retired()||work.reply.is_closed()||Instant::now()>=rpc_deadline{
                let reason=if Instant::now()>=rpc_deadline{RuntimeError::Timeout}else{RuntimeError::Cancelled};self.backend.retire_im_read(packets.call_id);work.interest.cancel();let _=work.reply.send(Err(reason));let _=command.completion.send(Err(reason));return Ok(());
            }
            self.pending.insert(packets.call_id,rpc_deadline);
            self.im_pending.insert(packets.call_id,ImReadPending{query:work.query,gate:work.gate,interest:work.interest,deadline:rpc_deadline,reply:work.reply});
            // Receipt says only that this READ frame was fully submitted. A
            // cancelled result wait now retires interest without replaying or
            // closing healthy SDK unregister/logout. It is not a peer ACK.
            if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),Some(packets.call_id)))).is_err(){self.retire_im_reads(Instant::now());}
            return Ok(());
        }
        if let Some(work)=received_wait{
            if !work.permit.submitted()||!work.permit.request_is_current(packets.call_id)||work.reply.is_closed(){
                let reason=if Instant::now()>=rpc_deadline{RuntimeError::Timeout}else{RuntimeError::Cancelled};work.permit.cancel();self.backend.retire_received_write(packets.call_id);let _=work.reply.send(Err(reason));let _=command.completion.send(Err(reason));return Err(reason);
            }
            self.pending.insert(packets.call_id,rpc_deadline);
            self.received_pending.insert(packets.call_id,ReceivedPending{permit:work.permit,deadline:rpc_deadline,reply:work.reply});
            if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),Some(packets.call_id)))).is_err(){self.retire_received_writes(Instant::now());return Err(RuntimeError::Cancelled);}return Ok(());
        }
        if let Some(work)=user_wait{
            if !work.permit.submitted()||!work.permit.current(packets.call_id)||work.reply.is_closed(){work.permit.cancel();self.backend.retire_user_read_write(packets.call_id);let _=work.reply.send(Err(RuntimeError::Cancelled));let _=command.completion.send(Err(RuntimeError::Cancelled));return Err(RuntimeError::Cancelled);}
            self.pending.insert(packets.call_id,rpc_deadline);self.user_read_pending.insert(packets.call_id,UserReadPending{permit:work.permit,deadline:rpc_deadline,reply:work.reply});
            if command.completion.send(Ok(RuntimeReceipt::new(self.owner.clone(),self.scope.clone(),Some(packets.call_id)))).is_err(){self.retire_user_read_writes(Instant::now());return Err(RuntimeError::Cancelled);}return Ok(());
        }
        if !is_response { self.pending.insert(packets.call_id,rpc_deadline); }
        if let Some((ticket,operation,completion))=call_wait{
            self.call_pending.insert(packets.call_id,CallPending{ticket,operation,deadline:rpc_deadline,completion});
        }
        if let Some(sequence) = unhosting_sequence {
            self.unhosting_pending = Some(UnhostingPending { sequence, outer_id: packets.call_id,
                deadline:rpc_deadline, completion: command.completion });
            return Ok(());
        }
        if is_logout { self.logout_pending = Some(LogoutPending { id: packets.call_id, completion: command.completion }); return Ok(()); }
        let mut receipt = RuntimeReceipt::new(self.owner.clone(), self.scope.clone(), Some(packets.call_id));
        // Only a completed original response WRITE can carry this local replay
        // boundary. BatchQueued/login/poll receipts cannot mint compaction proof.
        if is_response { receipt.direct = self.backend.direct_replay_frontier(); }
        if command.completion.send(Ok(receipt)).is_err() {
            return Err(RuntimeError::Cancelled);
        }
        Ok(())
    }
    async fn send_packets(&mut self,packets:&[super::transport_packets::TransportBytes],completion:Option<&mut oneshot::Sender<Result<RuntimeReceipt,RuntimeError>>>,native_deadline:Option<Instant>)->Result<(),RuntimeError>{
        if self.common.is_some(){return self.send_common_packets(packets,completion,native_deadline).await;}
        let wire=self.tls.as_mut().ok_or(RuntimeError::Closed)?.encrypt_packets(&self.owner,&self.scope,packets,self.timing,Instant::now())?.ok_or(RuntimeError::Closed)?;
        self.send_wire(&wire,completion).await
    }
    async fn send_wire(&mut self, wire: &[OwnedWireFrames], completion: Option<&mut oneshot::Sender<Result<RuntimeReceipt, RuntimeError>>>)
        -> Result<(), RuntimeError> {
        let mut frames = Vec::new(); let mut total = 0usize;
        for group in wire {
            for frame in group.frames_for(&self.owner, &self.scope).ok_or(RuntimeError::Closed)? {
                let bytes = frame.expose_to_transport(); total = total.checked_add(bytes.len()).ok_or(RuntimeError::Backpressure)?;
                if total > 2 * MAX_PARAMETER_BYTES + 4096 { return Err(RuntimeError::Backpressure); }
                frames.push(bytes);
            }
        }
        let step_deadline = self.pending.values().copied().chain(self.tls.as_ref().and_then(TlsEngine::next_deadline))
            .chain(self.unhosting_pending.as_ref().map(|pending| pending.deadline)).chain(self.call_pending.values().map(|pending|pending.deadline)).min();
        let deadline = Instant::now().checked_add(self.policy.io_timeout).ok_or(RuntimeError::Timeout)?;
        let deadline = step_deadline.map_or(deadline, |rpc| deadline.min(rpc));
        let now = Instant::now();
        if now >= deadline { self.scope.close(); return Err(RuntimeError::Timeout); }
        let timeout = deadline.saturating_duration_since(now);
        let scope = self.scope.clone();
        if self.tls.as_mut().ok_or(RuntimeError::Closed)?.tick(&self.owner, &self.scope, Instant::now()).is_err() { return Err(RuntimeError::Timeout); }
        let io = self.io.as_mut().ok_or(RuntimeError::Closed)?;
        let sending = write_frames(io, &scope, &frames);
        let result = if let Some(completion) = completion {
            tokio::select! { biased;
                _ = scope.closed() => Err(RuntimeError::Closed),
                _ = self.events.closed() => Err(RuntimeError::Closed),
                _ = completion.closed() => Err(RuntimeError::Cancelled),
                result = tokio::time::timeout(timeout, sending) => result.unwrap_or(Err(RuntimeError::Timeout)),
            }
        } else {
            tokio::select! { biased;
                _ = scope.closed() => Err(RuntimeError::Closed),
                _ = self.events.closed() => Err(RuntimeError::Closed),
                result = tokio::time::timeout(timeout, sending) => result.unwrap_or(Err(RuntimeError::Timeout)),
            }
        };
        if result.is_err() { self.scope.close(); }
        result
    }
}

/// Polls are nonblocking and original-scope-gated one at a time. Cancellation
/// cannot return this IO to a caller or reset/encrypt another request on it.
async fn write_frames<I: AsyncWrite + Unpin>(io: &mut I, scope: &RpcConnectionScope, frames: &[&[u8]]) -> Result<(), RuntimeError> {
    for frame in frames {
        let mut offset = 0usize;
        while offset < frame.len() {
            let count = poll_fn(|context| {
                match scope.with_io_gate(|| Pin::new(&mut *io).poll_write(context, &frame[offset..])) {
                    None => Poll::Ready(Err(io::Error::other("closed original scope"))),
                    Some(result) => result,
                }
            }).await.map_err(|_| RuntimeError::Io)?;
            if count == 0 { return Err(RuntimeError::Io); }
            if count > frame.len() - offset { return Err(RuntimeError::Io); }
            offset += count;
        }
    }
    poll_fn(|context| match scope.with_io_gate(|| Pin::new(&mut *io).poll_flush(context)) {
        None => Poll::Ready(Err(io::Error::other("closed original scope"))), Some(result) => result,
    }).await.map_err(|_| RuntimeError::Io)
}

#[path="arc_native_actor.rs"]
pub(super) mod native;

#[cfg(test)]
#[path = "transport_actor_tests.rs"]
mod tests;
