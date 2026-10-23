//! Serialized original-IO/TLS/ARC runtime for caller-provided verified routing.
//!
//! Never dials, resolves, installs a provider or grants channels. The caller
//! supplies a nonblocking AsyncRead+AsyncWrite object and exact imported TLS/
//! route/login inputs. One actor owns that IO and all TLS/ARC state. Scope close
//! serializes with every write/flush poll; partial/cancelled writes terminate,
//! not replay or rebind. Bytes already accepted by IO/kernel cannot be recalled.
use super::{
    arc_session::{ArcLoginInputs, ArcPhase},
    offline_backend::{OfflineBackend, OfflineEvent},
    packet_transport::{OwnedWireFrames, PacketRoute, RouterTiming},
    rpc_incoming::IncomingBinding,
    rpc_pending::RpcConnectionScope,
    rpc_stream::RpcProperty,
    session::{AttemptOwner, GatewayRegistration},
    tls_engine::{TlsEngine, TlsError, TlsParameters, TlsPhase},
    batch_scheduler::ServerRequestTicket,
    arc_protocol::DirectReplyTicket,
};
use std::{fmt, future::poll_fn, io, pin::Pin, task::Poll, time::{Duration, Instant}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite}, sync::{mpsc, oneshot}, task::JoinHandle};
use zeroize::{Zeroize, Zeroizing};

const MAX_COMMANDS: usize = 32;
const MAX_EVENTS: usize = 128;
const MAX_PARAMETER_BYTES: usize = 128 * 1024;
const MAX_PENDING_RPC: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError { Scope, InvalidConfiguration, Backpressure, Closed, Cancelled, Timeout, Io, Protocol, LoggedOut, Tls(TlsError) }
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
    Login(Login, Vec<Property>), Bootstrap(Zeroizing<Vec<u8>>, Vec<Property>), Configure(u32, Option<Binding>), Logout(Vec<Property>),
    Swap(Vec<Property>, Option<Gateway>), CompleteBatch(ServerRequestTicket, bool), CompleteDirect(DirectReplyTicket, bool),
}
impl Operation {
    fn byte_length(&self) -> usize {
        match self {
            Self::Login(login, routing) => [&login.identity, &login.domain, &login.selector, &login.phone, &login.key,
                &login.domain_id, &login.application_id, &login.cc].iter().map(|v| v.len()).sum::<usize>()
                + login.properties.iter().chain(routing).map(|p| p.key.len() + p.value.len()).sum::<usize>(),
            Self::Bootstrap(key, routing) => key.len() + routing.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>(),
            Self::Logout(routing) => routing.iter().map(|p| p.key.len() + p.value.len()).sum(),
            Self::Configure(_, incoming) => incoming.as_ref().map_or(0, |b| b.object.len() + b.domain.len() + b.receiver.len()),
            Self::Swap(routing, gateway) => routing.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>()
                + gateway.as_ref().map_or(0, |g| g.uri.len() + g.prefix.len() + g.channel.len() + g.int_code.len() + g.number.len() + g.password.len()
                    + g.metadata.iter().map(|p| p.key.len() + p.value.len()).sum::<usize>()),
            Self::CompleteBatch(_, _) | Self::CompleteDirect(_, _) => 0,
        }
    }
}
struct Command { operation: Operation, completion: oneshot::Sender<Result<RuntimeReceipt, RuntimeError>> }
struct LogoutPending { id: i32, completion: oneshot::Sender<Result<RuntimeReceipt, RuntimeError>> }
struct LogoutRetirement(RpcConnectionScope);
impl Drop for LogoutRetirement { fn drop(&mut self) { self.0.close(); } }

pub struct RuntimeReceipt { owner: AttemptOwner, scope: RpcConnectionScope, pub call_id: Option<i32> }
impl RuntimeReceipt {
    /// Successful transport submission is NOT an ARC login or hosting ACK.
    pub fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
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
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(&self.events) } else { None }
    }
    pub fn into_events(self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<Vec<OfflineEvent>> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(self.events) } else { None }
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
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    async fn submit(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, operation: Operation) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        if operation.byte_length() > MAX_PARAMETER_BYTES { return Err(RuntimeError::InvalidConfiguration); }
        let (completion, result) = oneshot::channel();
        self.commands.try_send(Command { operation, completion }).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => RuntimeError::Backpressure,
            mpsc::error::TrySendError::Closed(_) => RuntimeError::Closed,
        })?;
        result.await.map_err(|_| RuntimeError::Closed)?
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
        self.submit(owner, scope, Operation::Swap(Property::capture(routing)?, Some(Gateway::capture(uri, status, registration, metadata)?))).await
    }
    pub async fn complete_batch(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &ServerRequestTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteBatch(ticket.captured_copy(), accepted)).await
    }
    pub async fn complete_direct(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, ticket: &DirectReplyTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        if !self.accepts(owner, scope) || !ticket.matches(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::CompleteDirect(ticket.captured_copy(), accepted)).await
    }
    /// Unlike login/hosting submission receipts, this wait completes ONLY on
    /// the matched original logout RPC result. It does not own/join the IO task.
    pub async fn logout(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, routing: &[RpcProperty<'_>]) -> Result<(), RuntimeError> {
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        let routing = logout_properties(routing)?;
        let _retirement = LogoutRetirement(self.scope.clone());
        self.submit(owner, scope, Operation::Logout(routing)).await.map(|_| ())
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
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
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
        if !self.accepts(owner, scope) { return Err(RuntimeError::Scope); }
        self.submit(owner, scope, Operation::Swap(Property::capture(routing)?, Some(Gateway::capture(uri, status, registration, metadata)?))).await
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

/// Start only on caller-provided nonblocking IO, never a guessed network target.
/// This low-level runtime is not installed as a ready HostingBackend/provider.
pub fn start<I>(io: I, owner: AttemptOwner, route: PacketRoute<'_>, tls_parameters: TlsParameters<'_>,
    policy: RuntimePolicy, timing: Option<RouterTiming>) -> Result<(RuntimeHandle, mpsc::Receiver<RuntimeEvent>), RuntimeError>
    where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    if !policy.valid() { return Err(RuntimeError::InvalidConfiguration); }
    let start = Instant::now(); let mut backend = OfflineBackend::new(owner.clone()); let scope = backend.connection_scope();
    let tls = backend.take_tls_engine(route, tls_parameters, start)?;
    let (commands_tx, commands_rx) = mpsc::channel(MAX_COMMANDS); let (events_tx, events_rx) = mpsc::channel(MAX_EVENTS);
    let actor = Actor { io: Some(io), owner: owner.clone(), scope: scope.clone(), backend, tls, commands: commands_rx,
        events: events_tx, policy, timing, start, pending: std::collections::BTreeMap::new(), logout_pending: None };
    let task = tokio::spawn(actor.run());
    Ok((RuntimeHandle { owner, scope, commands: commands_tx, task: Some(task) }, events_rx))
}

struct Actor<I> {
    io: Option<I>, owner: AttemptOwner, scope: RpcConnectionScope, backend: OfflineBackend, tls: TlsEngine,
    commands: mpsc::Receiver<Command>, events: mpsc::Sender<RuntimeEvent>, policy: RuntimePolicy,
    timing: Option<RouterTiming>, start: Instant, pending: std::collections::BTreeMap<i32, Instant>, logout_pending: Option<LogoutPending>,
}
impl<I> Drop for Actor<I> {
    fn drop(&mut self) { self.scope.close(); self.backend.close(); self.tls.close(); self.io = None; self.commands.close(); }
}
impl<I> Actor<I> where I: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    async fn run(mut self) -> RuntimeError {
        let result = self.loop_run().await;
        self.scope.close(); self.backend.close(); self.tls.close(); self.io = None; self.commands.close();
        while let Ok(command) = self.commands.try_recv() { let _ = command.completion.send(Err(RuntimeError::Closed)); }
        result
    }
    fn elapsed_ms(&self, now: Instant) -> u64 { now.saturating_duration_since(self.start).as_millis().min(u64::MAX as u128) as u64 }
    async fn loop_run(&mut self) -> RuntimeError {
        let initial = match self.tls.take_wire_output(&self.owner, &self.scope, self.timing, Instant::now()) {
            Ok(initial) => initial, Err(error) => return error.into(),
        };
        if let Err(error) = self.send_wire(&initial, None).await { return error; }
        let mut read = [0; 8192]; let mut timer = tokio::time::interval(Duration::from_millis(20));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if !self.scope.is_open() { return RuntimeError::Closed; }
            tokio::select! {
                biased;
                _ = self.scope.closed() => return RuntimeError::Closed,
                _ = self.events.closed() => return RuntimeError::Closed,
                _ = timer.tick() => {
                    let now = Instant::now();
                    if let Err(error) = self.tls.tick(&self.owner, &self.scope, now) { return error.into(); }
                    self.pending.retain(|id, _| self.backend.is_pending(*id));
                    if self.logout_pending.as_ref().is_some_and(|pending| pending.completion.is_closed()) { return RuntimeError::Cancelled; }
                    if self.pending.values().any(|deadline| now >= *deadline) { return RuntimeError::Timeout; }
                }
                received = self.io.as_mut().expect("owned IO").read(&mut read) => {
                    let count = match received { Ok(0) => { let _ = self.tls.finish_eof(&self.owner, &self.scope); return RuntimeError::Io; },
                        Ok(count) => count, Err(_) => return RuntimeError::Io };
                    let now = Instant::now();
                    let progress = match self.tls.on_wire_bytes(&self.owner, &self.scope, &read[..count], self.timing, now) {
                        Ok(Some(progress)) => progress, Ok(None) => return RuntimeError::Closed, Err(error) => return error.into(),
                    };
                    if let Err(error) = self.send_wire(&progress.wire, None).await { return error; }
                    let elapsed_ms = self.elapsed_ms(now);
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
        let mut visible = Vec::new();
        for event in events {
            match event {
                OfflineEvent::Logout(reply) => {
                    let pending = self.logout_pending.take().ok_or(RuntimeError::Protocol)?;
                    let accepted = reply.matches(&self.owner, &self.scope) && reply.call_id() == pending.id && reply.accepted();
                    let result = if accepted { Ok(RuntimeReceipt { owner: self.owner.clone(), scope: self.scope.clone(), call_id: Some(pending.id) }) }
                        else { Err(RuntimeError::Protocol) };
                    if pending.completion.send(result).is_err() { return Err(RuntimeError::Cancelled); }
                    return Err(if accepted { RuntimeError::LoggedOut } else { RuntimeError::Protocol });
                }
                OfflineEvent::Outgoing(packets) => {
                    let wire = self.tls.encrypt_packets(&self.owner, &self.scope, &packets.packets, self.timing, Instant::now())?.ok_or(RuntimeError::Closed)?;
                    self.send_wire(&wire, None).await?;
                }
                event => visible.push(event),
            }
        }
        if !visible.is_empty() {
            self.events.try_send(RuntimeEvent { owner: self.owner.clone(), scope: self.scope.clone(), events: visible })
                .map_err(|_| RuntimeError::Backpressure)?;
        }
        Ok(())
    }
    async fn command(&mut self, mut command: Command) -> Result<(), RuntimeError> {
        if !self.scope.is_open() { let _ = command.completion.send(Err(RuntimeError::Closed)); return Err(RuntimeError::Closed); }
        let now = Instant::now();
        if self.logout_pending.is_some() { let _ = command.completion.send(Err(RuntimeError::Closed)); return Ok(()); }
        if self.tls.phase() != TlsPhase::Verified {
            let _ = command.completion.send(Err(RuntimeError::Tls(TlsError::NotReady))); return Ok(());
        }
        if self.pending.len() >= MAX_PENDING_RPC && matches!(&command.operation, Operation::Login(_, _) | Operation::Bootstrap(_, _) | Operation::Swap(_, _)) {
            let _ = command.completion.send(Err(RuntimeError::Backpressure)); return Ok(());
        }
        let is_response = matches!(&command.operation, Operation::CompleteDirect(_, _));
        let is_logout = matches!(&command.operation, Operation::Logout(_));
        let elapsed_ms = self.elapsed_ms(now);
        let prepared = match command.operation {
            Operation::Configure(epoch, incoming) => {
                let result = self.backend.configure(epoch, incoming.as_ref().map(Binding::borrowed)).map_err(|_| RuntimeError::Protocol);
                let receipt = result.map(|()| RuntimeReceipt { owner: self.owner.clone(), scope: self.scope.clone(), call_id: None });
                if command.completion.send(receipt).is_err() { return Err(RuntimeError::Cancelled); }
                return Ok(());
            }
            Operation::CompleteBatch(ticket, accepted) => {
                let result = self.backend.complete_message(&ticket, accepted).and_then(|completed| {
                    if completed { Ok(()) } else { Err(super::arc_protocol::ProtocolError::Rpc(super::rpc_stream::CodecError::InvalidField)) }
                }).map_err(|_| RuntimeError::Protocol);
                if command.completion.send(result.map(|()| RuntimeReceipt { owner: self.owner.clone(), scope: self.scope.clone(), call_id: None })).is_err() {
                    return Err(RuntimeError::Cancelled);
                }
                return Ok(());
            }
            Operation::CompleteDirect(ticket, accepted) => match self.backend.complete_direct_message(&ticket, accepted) {
                Ok(Some(packets)) => Ok(packets),
                _ => { let _ = command.completion.send(Err(RuntimeError::Protocol)); return Ok(()); }
            },
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
        if is_logout { self.pending.retain(|id, _| self.backend.is_pending(*id)); }
        let wire = match self.tls.encrypt_packets(&self.owner, &self.scope, &packets.packets, self.timing, now) {
            Ok(Some(wire)) => wire,
            Ok(None) => { let _ = command.completion.send(Err(RuntimeError::Closed)); return Err(RuntimeError::Closed); }
            Err(error) => { let _ = command.completion.send(Err(error.into())); return Err(error.into()); }
        };
        if let Err(error) = self.send_wire(&wire, Some(&mut command.completion)).await {
            self.backend.retire_request(packets.call_id); let _ = command.completion.send(Err(error)); return Err(error);
        }
        if !is_response {
            self.pending.insert(packets.call_id, Instant::now().checked_add(self.policy.rpc_timeout).ok_or(RuntimeError::Timeout)?);
        }
        if is_logout { self.logout_pending = Some(LogoutPending { id: packets.call_id, completion: command.completion }); return Ok(()); }
        if command.completion.send(Ok(RuntimeReceipt { owner: self.owner.clone(), scope: self.scope.clone(), call_id: Some(packets.call_id) })).is_err() {
            return Err(RuntimeError::Cancelled);
        }
        Ok(())
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
        let step_deadline = self.pending.values().copied().chain(self.tls.next_deadline()).min();
        let deadline = Instant::now().checked_add(self.policy.io_timeout).ok_or(RuntimeError::Timeout)?;
        let deadline = step_deadline.map_or(deadline, |rpc| deadline.min(rpc));
        let timeout = deadline.saturating_duration_since(Instant::now());
        let scope = self.scope.clone();
        if self.tls.tick(&self.owner, &self.scope, Instant::now()).is_err() { return Err(RuntimeError::Timeout); }
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

#[cfg(test)]
#[path = "transport_actor_tests.rs"]
mod tests;
