//! Explicit, original-attempt native driver. Not installed as a global provider.
//!
//! Original terminal authentication/hosting failures retain their private
//! actor-produced reason even when ARC has already retired the physical scope.
//! Malformed data remains a protocol failure, never a fabricated native reject.
//! A supplied profile is deployment data, never account/channel readiness. Only
//! DirectTlsCarrier is supported, either explicitly or through a matched
//! bootstrap-to-account handoff with separately imported TLS trust. No default
//! target/root/identity, paid outbound, automatic consumer ACK or Channels event.
use super::{
    arc_protocol::{DirectReplyTicket, IncomingProtocolMessage},
    arc_session::ArcLoginInputs,
    batch_scheduler::ServerRequestTicket,
    controller::Callback,
    call_binding::CallIncomingBinding,
    call_delivery::{CallReplyTicket,IncomingCallDelivery},
    call_control::{CallControlTicket,OwnedCallOperationReply},
    rpc_calls::CallOperation,
    legacy_config::ConfigDecodeKey,
    offline_backend::{OfflineEvent, ScheduledIncomingMessage},
    packet_transport::PacketRoute,
    rpc_pending::{OwnedRpcCallback, RpcConnectionScope},
    rpc_stream::{RpcProperty, MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
    runtime_connector::{ConnectionInput, RuntimeConnector},
    runtime_provision::{AccountTrust, ProvisionedLogin},
    session::{AttemptOwner, BackendFailure, BackendFuture, GatewayRegistration, HostingBackend,
        SdkLoginRequest, STATUS_REGISTER, STATUS_UNREGISTER},
    tls_engine::{valid_authority, TlsError, TlsParameters},
    transport_actor::{self, RuntimeCommands, RuntimeError, RuntimeEvent, RuntimeHandle, RuntimePolicy, RuntimeReceipt},
};
use ring::rand::{SecureRandom, SystemRandom};
use std::{fmt, sync::{Arc, Mutex, MutexGuard}, time::{Duration, Instant}};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use zeroize::{Zeroize, Zeroizing};

const MAX_PROFILE_BYTES: usize = 128 * 1024;
const MAX_ATTEMPTS: usize = 1024;
const EVENT_CAPACITY: usize = 65; // One slot reserved exclusively for Terminal.
const POLL_CADENCE: Duration = Duration::from_millis(250);
const TLS_RETRY_CADENCE: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError { InvalidConfiguration, UnsupportedTransport, ResourceLimit }

/// All deployment values must be supplied explicitly. Common's decode key is
/// moved into the profile; it is NOT a password/application-key substitute.
pub struct RuntimeProfileInputs<'a> {
    pub route: PacketRoute<'a>,
    pub authority: &'a str,
    pub enable_sni: bool,
    pub identity_type: &'a str,
    pub domain_name: &'a str,
    pub domain_id: &'a str,
    pub application_id: &'a str,
    pub device_info_cc: &'a str,
    pub client_properties: &'a [RpcProperty<'a>],
    /// Merge only the actual SDK request's DeviceId, never a profile replacement.
    pub include_device_id: bool,
    pub login_routing: &'a [RpcProperty<'a>],
    pub gateway_uri: &'a str,
    pub gateway_metadata: &'a [RpcProperty<'a>],
    pub poll_routing: &'a [RpcProperty<'a>],
    pub logout_routing: &'a [RpcProperty<'a>],
    pub incoming: Option<super::rpc_incoming::IncomingBinding<'a>>,
    pub reference_decode_key: Option<ConfigDecodeKey>,
    pub handshake_timeout: Duration,
    pub partial_timeout: Duration,
    pub io_timeout: Duration,
    pub rpc_timeout: Duration,
}
impl fmt::Debug for RuntimeProfileInputs<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeProfileInputs(<redacted>)") }
}
struct Property { key: Vec<u8>, value: Vec<u8> }
impl Drop for Property { fn drop(&mut self) { self.key.zeroize(); self.value.zeroize(); } }
fn properties(input: &[RpcProperty<'_>], total: &mut usize) -> Result<Vec<Property>, ProfileError> {
    if input.len() > MAX_MAP_ENTRIES { return Err(ProfileError::ResourceLimit); }
    for (index, p) in input.iter().enumerate() {
        if p.key.is_empty() || p.key.len() > MAX_FIELD_BYTES || p.value.len() > MAX_FIELD_BYTES
            || input[..index].iter().any(|prior| prior.key == p.key) {
            return Err(ProfileError::InvalidConfiguration);
        }
        add_bytes(total, p.key.len())?; add_bytes(total, p.value.len())?;
    }
    Ok(input.iter().map(|p| Property { key: p.key.to_vec(), value: p.value.to_vec() }).collect())
}
fn borrowed(input: &[Property]) -> Vec<RpcProperty<'_>> {
    input.iter().map(|p| RpcProperty { key: &p.key, value: &p.value }).collect()
}
fn add_bytes(total: &mut usize, bytes: usize) -> Result<(), ProfileError> {
    *total = total.checked_add(bytes).ok_or(ProfileError::ResourceLimit)?;
    if *total > MAX_PROFILE_BYTES { return Err(ProfileError::ResourceLimit); }
    Ok(())
}
fn text(value: &str, empty: bool, total: &mut usize) -> Result<Zeroizing<String>, ProfileError> {
    if (!empty && value.trim().is_empty()) || value.len() > MAX_FIELD_BYTES
        || value.chars().any(char::is_control) || value.contains("ENC(") {
        return Err(ProfileError::InvalidConfiguration);
    }
    add_bytes(total, value.len())?; Ok(Zeroizing::new(value.to_owned()))
}
fn reference_text(value: &str, total: &mut usize) -> Result<Zeroizing<String>, ProfileError> {
    // Original Common reference ENC is decoded only by the explicit connector
    // key context; it is not a password or recursively decoded client property.
    if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control) {
        return Err(ProfileError::InvalidConfiguration);
    }
    add_bytes(total, value.len())?; Ok(Zeroizing::new(value.to_owned()))
}
fn routing(input: &[RpcProperty<'_>], total: &mut usize) -> Result<Vec<Property>, ProfileError> {
    if input.iter().any(|p| p.key == b"pwd" || p.key == b"token") { return Err(ProfileError::InvalidConfiguration); }
    properties(input, total)
}
struct Binding { object: Zeroizing<Vec<u8>>, domain: Zeroizing<Vec<u8>>, receiver: Zeroizing<Vec<u8>> }
impl Binding {
    fn borrowed(&self) -> super::rpc_incoming::IncomingBinding<'_> {
        super::rpc_incoming::IncomingBinding { object_id: &self.object, domain: &self.domain, receiver_kind: &self.receiver }
    }
}

struct CallBinding { object:Zeroizing<Vec<u8>>, domain:Zeroizing<Vec<u8>> }
impl CallBinding { fn borrowed(&self)->CallIncomingBinding<'_>{CallIncomingBinding{object_id:&self.object,domain:&self.domain}} }

pub struct RuntimeBootstrapInputs<'a> {
    pub account_trust: AccountTrust,
    pub routing: &'a [RpcProperty<'a>],
    pub client_config_decode_key: Option<ConfigDecodeKey>,
    pub handoff_timeout: Duration,
}
impl fmt::Debug for RuntimeBootstrapInputs<'_> {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RuntimeBootstrapInputs(<redacted>)")}
}
struct BootstrapProfile {
    account_trust: AccountTrust, routing:Vec<Property>, client_key:Option<ConfigDecodeKey>, handoff_timeout:Duration,
}

/// Bounded, owned, redacted, zeroizing deployment data. No Serialize/Default.
pub struct RuntimeProfile {
    authority: Zeroizing<String>, enable_sni: bool, identity_type: Zeroizing<String>,
    domain_name: Zeroizing<String>, domain_id: Zeroizing<String>, application_id: Zeroizing<String>,
    device_info_cc: Zeroizing<String>, client_properties: Vec<Property>, include_device_id: bool,
    login_routing: Vec<Property>, gateway_uri: Zeroizing<String>, gateway_metadata: Vec<Property>,
    poll_routing: Vec<Property>, logout_routing: Vec<Property>, incoming: Option<Binding>,
    reference_decode_key: Option<ConfigDecodeKey>, handshake_timeout: Duration, partial_timeout: Duration,
    io_timeout: Duration, rpc_timeout: Duration, bootstrap:Option<BootstrapProfile>, call_incoming:Option<CallBinding>,call_control:Option<Vec<Property>>, byte_length:usize,
}
impl fmt::Debug for RuntimeProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeProfile(<redacted>)") }
}
impl RuntimeProfile {
    pub fn with_call_receiver(mut self,binding:Option<CallIncomingBinding<'_>>)->Result<Self,ProfileError>{
        if self.call_incoming.is_some(){return Err(ProfileError::InvalidConfiguration);}
        if let Some(binding)=binding{
            for value in [binding.object_id,binding.domain]{
                if value.is_empty()||value.len()>MAX_FIELD_BYTES||value.contains(&b'*')||value.iter().any(u8::is_ascii_control)||value.windows(4).any(|w|w==b"ENC("){return Err(ProfileError::InvalidConfiguration);}
                add_bytes(&mut self.byte_length,value.len())?;
            }
            self.call_incoming=Some(CallBinding{object:Zeroizing::new(binding.object_id.to_vec()),domain:Zeroizing::new(binding.domain.to_vec())});
        }
        Ok(self)
    }
    pub fn with_call_control(mut self,routing:&[RpcProperty<'_>])->Result<Self,ProfileError>{
        if self.call_incoming.is_none()||self.call_control.is_some(){return Err(ProfileError::InvalidConfiguration);}
        super::call_control::validate_routing(routing).map_err(|_|ProfileError::InvalidConfiguration)?;
        self.call_control=Some(properties(routing,&mut self.byte_length)?);Ok(self)
    }
    pub fn supplied(input: RuntimeProfileInputs<'_>) -> Result<Self, ProfileError> { Self::capture(input,None) }
    /// Exact imported bootstrap TLS anchor plus separately imported account
    /// trust. Login domain/app/CC must come from the matched bootstrap, never
    /// placeholder/manual values silently substituted for returned properties.
    pub fn supplied_bootstrap(input:RuntimeProfileInputs<'_>,bootstrap:RuntimeBootstrapInputs<'_>)->Result<Self,ProfileError>{
        if [input.domain_name,input.domain_id,input.application_id,input.device_info_cc].iter().any(|value|!value.is_empty())
            ||bootstrap.handoff_timeout.is_zero()||bootstrap.handoff_timeout>Duration::from_secs(30){return Err(ProfileError::InvalidConfiguration);}
        Self::capture(input,Some(bootstrap))
    }
    fn capture(input:RuntimeProfileInputs<'_>,bootstrap:Option<RuntimeBootstrapInputs<'_>>)->Result<Self,ProfileError>{
        if !matches!(input.route, PacketRoute::DirectTlsCarrier) { return Err(ProfileError::UnsupportedTransport); }
        if !valid_authority(input.authority) || input.identity_type.is_empty()
            || input.identity_type.len() > 64
            || !input.identity_type.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
            return Err(ProfileError::InvalidConfiguration);
        }
        for (timeout, maximum) in [(input.handshake_timeout, 30), (input.partial_timeout, 30),
            (input.io_timeout, 30), (input.rpc_timeout, 60)] {
            if timeout.is_zero() || timeout > Duration::from_secs(maximum) { return Err(ProfileError::InvalidConfiguration); }
        }
        // ClientId is an exact supplied native property. No fabricated prefix.
        // DeviceId, pwd and token must originate from the actual SDK request.
        if !input.client_properties.iter().any(|p| p.key == b"ClientId" && !p.value.is_empty())
            || input.client_properties.iter().any(|p| p.key == b"DeviceId" || p.key == b"pwd" || p.key == b"token"
                || p.key == b"ForceFlag" && p.value != b"0") {
            return Err(ProfileError::InvalidConfiguration);
        }
        let mut total = 0;
        let from_bootstrap=bootstrap.is_some();
        if !from_bootstrap && input.domain_name.bytes().any(|b| matches!(b, b'@' | b'[' | b']')) {
            return Err(ProfileError::InvalidConfiguration);
        }
        let bootstrap=bootstrap.map(|bootstrap| -> Result<BootstrapProfile, ProfileError> {
            add_bytes(&mut total,bootstrap.account_trust.authority().len())?;
            add_bytes(&mut total,bootstrap.account_trust.root_der_base64().len())?;
            let captured=routing(bootstrap.routing,&mut total)?;
            Ok(BootstrapProfile{account_trust:bootstrap.account_trust,routing:captured,client_key:bootstrap.client_config_decode_key,handoff_timeout:bootstrap.handoff_timeout})
        }).transpose()?;
        let incoming = input.incoming.map(|b| {
            for value in [b.object_id, b.domain, b.receiver_kind] {
                if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.iter().any(u8::is_ascii_control)
                    || value.windows(4).any(|w| w == b"ENC(") { return Err(ProfileError::InvalidConfiguration); }
                add_bytes(&mut total, value.len())?;
            }
            if b.receiver_kind.contains(&b'*') { return Err(ProfileError::InvalidConfiguration); }
            Ok(Binding { object: Zeroizing::new(b.object_id.to_vec()), domain: Zeroizing::new(b.domain.to_vec()),
                receiver: Zeroizing::new(b.receiver_kind.to_vec()) })
        }).transpose()?;
        Ok(Self {
            authority: text(input.authority, false, &mut total)?, enable_sni: input.enable_sni,
            identity_type: text(input.identity_type, false, &mut total)?,
            domain_name: text(input.domain_name, from_bootstrap, &mut total)?, domain_id: text(input.domain_id, from_bootstrap, &mut total)?,
            application_id: text(input.application_id, from_bootstrap, &mut total)?, device_info_cc: text(input.device_info_cc, true, &mut total)?,
            client_properties: properties(input.client_properties, &mut total)?, include_device_id: input.include_device_id,
            login_routing: routing(input.login_routing, &mut total)?, gateway_uri: text(input.gateway_uri, false, &mut total)?,
            gateway_metadata: routing(input.gateway_metadata, &mut total)?, poll_routing: routing(input.poll_routing, &mut total)?,
            logout_routing: routing(input.logout_routing, &mut total)?, incoming, reference_decode_key: input.reference_decode_key,
            handshake_timeout: input.handshake_timeout, partial_timeout: input.partial_timeout,
            io_timeout: input.io_timeout, rpc_timeout: input.rpc_timeout, bootstrap, call_incoming:None,call_control:None,byte_length:total,
        })
    }
}

struct LoginCapture {
    identity: Zeroizing<String>, password: Zeroizing<String>, appkey: Zeroizing<String>,
    reference: Zeroizing<String>, root: Zeroizing<String>, selector: Zeroizing<String>, device: Option<Zeroizing<String>>,
}
impl fmt::Debug for LoginCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LoginCapture(<redacted>)") }
}
impl LoginCapture {
    fn capture(request: SdkLoginRequest<'_>, profile: &RuntimeProfile) -> Result<Self, BackendFailure> {
        if !request.tls_only || request.account_identity.bytes().any(|b| matches!(b, b':' | b'[' | b']' | b'@'))
            || request.device_id.is_some() && !profile.include_device_id {
            return Err(BackendFailure::Unavailable);
        }
        let mut total = 0;
        let invalid = |_| BackendFailure::Unavailable;
        let identity = text(request.account_identity, false, &mut total).map_err(invalid)?;
        let typed = Zeroizing::new(format!("{}:{}", profile.identity_type.as_str(), identity.as_str()));
        if typed.len() > MAX_FIELD_BYTES { return Err(BackendFailure::Unavailable); }
        Ok(Self { identity: typed,
            password: text(request.phone_login_token, false, &mut total).map_err(invalid)?,
            appkey: text(request.application_key, false, &mut total).map_err(invalid)?,
            reference: reference_text(request.server_address, &mut total).map_err(invalid)?,
            root: text(request.root_der_base64, false, &mut total).map_err(invalid)?,
            selector: text(request.selected_session_id.filter(|s| s.len() <= 127).ok_or(BackendFailure::Unavailable)?, false, &mut total).map_err(invalid)?,
            device: request.device_id.map(|d| text(d, false, &mut total)).transpose().map_err(invalid)?,
        })
    }
    fn connection<'a>(&'a self, profile: &'a RuntimeProfile) -> ConnectionInput<'a> {
        ConnectionInput { server_reference: &self.reference, authority: &profile.authority, root_der_base64: &self.root,
            enable_sni: profile.enable_sni, reference_decode_key: profile.reference_decode_key.as_ref() }
    }
    fn login<'a>(&'a self, profile: &'a RuntimeProfile, properties: &'a [RpcProperty<'a>]) -> ArcLoginInputs<'a> {
        ArcLoginInputs { typed_identity: self.identity.as_bytes(), domain_name: profile.domain_name.as_bytes(),
            selected_session_id: self.selector.as_bytes(), phone_login_token: self.password.as_bytes(),
            application_key: self.appkey.as_bytes(), domain_id: profile.domain_id.as_bytes(),
            application_id: profile.application_id.as_bytes(), device_info_cc: profile.device_info_cc.as_bytes(), client_properties: properties }
    }
}

/// Owned events retain ORIGINAL correlation even after terminal retirement.
/// A copied callback/message is not a channel or an open-scope lease.
pub struct BackendEvent { owner: AttemptOwner, scope: RpcConnectionScope, pub payload: BackendEventPayload }
pub enum BackendEventPayload {
    Callback(OwnedRpcCallback),
    BatchMessage(ScheduledIncomingMessage),
    DirectMessage(IncomingProtocolMessage),
    Call(IncomingCallDelivery),
    Terminal(BackendFailure),
}
impl BackendEvent {
    pub fn owner(&self) -> &AttemptOwner { &self.owner }
    pub fn connection_scope(&self) -> RpcConnectionScope { self.scope.clone() }
    pub fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope)
    }
}
impl fmt::Debug for BackendEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("BackendEvent(<redacted>)") }
}
impl fmt::Debug for BackendEventPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("BackendEventPayload(<redacted>)") }
}

#[derive(Default)]
struct AttemptState {
    retired: bool, configured: bool, unregister_started: bool, unregister_result: Option<Result<(), BackendFailure>>,
    logout_started: bool, logout_result: Option<Result<(), BackendFailure>>, released: bool,
}
struct Attempt {
    owner: AttemptOwner, scope: RpcConnectionScope, incarnation: u32,
    state: Mutex<AttemptState>, commands: Mutex<Option<RuntimeCommands>>,
    // Commands do not own IO. The owning handle is NEVER moved into a cleanup
    // future: cancellation of close retains its original Join in this slot.
    handle: AsyncMutex<Option<RuntimeHandle>>,
    bootstrap_scope: Option<RpcConnectionScope>,
    bootstrap_handle: AsyncMutex<Option<RuntimeHandle>>,
    // Serializes login/attach and controller commands with resource cleanup.
    // The independent event pump polls/configures without a controller lock.
    work: AsyncMutex<()>,
    // Polls remain live while status3 waits for a later nested result, but
    // their ADMISSION is serialized with starting SDK logout.
    poll_work: AsyncMutex<()>,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> { mutex.lock().unwrap_or_else(|p| p.into_inner()) }
impl Attempt {
    fn retire(&self) {
        lock(&self.state).retired = true; self.scope.close();
        if let Some(scope)=&self.bootstrap_scope{scope.close();}
    }
    fn commands(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Result<RuntimeCommands, RuntimeError> {
        if &self.owner != owner || !self.scope.matches(scope) || !self.scope.is_open() || lock(&self.state).retired {
            return Err(RuntimeError::Scope);
        }
        lock(&self.commands).clone().ok_or(RuntimeError::Closed)
    }
}
// Armed across connect/login/unregister/poll waits. Failed or cancelled work
// cannot leave an admissible scope. Consumer queue pressure is NOT armed.
struct Retirement { attempt: Arc<Attempt>, armed: bool }
impl Retirement {
    fn new(attempt: Arc<Attempt>) -> Self { Self { attempt, armed: true } }
    fn disarm(&mut self) { self.armed = false; }
}
impl Drop for Retirement { fn drop(&mut self) { if self.armed { self.attempt.retire(); } } }
struct BackendState { slot: Option<Arc<Attempt>>, owners: Vec<AttemptOwner>, incarnations: Vec<u32> }

pub struct NativeHostingBackend {
    profile: Arc<RuntimeProfile>, connector: Arc<dyn RuntimeConnector>, events: mpsc::Sender<BackendEvent>, state: Mutex<BackendState>,
}
impl fmt::Debug for NativeHostingBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeHostingBackend(<redacted>)") }
}
impl NativeHostingBackend {
    /// Explicit accepted profile only. Construction neither dials nor grants
    /// availability. No global provider/descriptor is altered by this factory.
    pub fn new(profile: RuntimeProfile, connector: Arc<dyn RuntimeConnector>) -> (Arc<Self>, mpsc::Receiver<BackendEvent>) {
        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        (Arc::new(Self { profile: Arc::new(profile), connector, events,
            state: Mutex::new(BackendState { slot: None, owners: Vec::new(), incarnations: Vec::new() }) }), receiver)
    }
    /// The trusted account wrapper owns this backend exclusively. Cancellation
    /// may happen before login returned its owner; retire that already-captured
    /// slot synchronously, without transferring or claiming release of its IO.
    pub(super) fn retire_current(&self) {
        if let Some(attempt) = &lock(&self.state).slot { attempt.retire(); }
    }
    pub(super) fn original_scope(&self, owner: &AttemptOwner) -> Option<RpcConnectionScope> {
        self.original(owner).ok().map(|attempt| attempt.scope.clone())
    }
    pub(super) fn original_incoming_binding(&self, owner: &AttemptOwner, scope: &RpcConnectionScope)
        -> Option<super::rpc_incoming::IncomingBinding<'_>> {
        let attempt = self.original(owner).ok()?;
        if !attempt.scope.matches(scope) || !scope.is_open() || !lock(&attempt.state).configured { return None; }
        self.profile.incoming.as_ref().map(Binding::borrowed)
    }
    fn original(&self, owner: &AttemptOwner) -> Result<Arc<Attempt>, BackendFailure> {
        lock(&self.state).slot.as_ref().filter(|a| &a.owner == owner).cloned().ok_or(BackendFailure::Disconnected)
    }
    /// Borrowed tickets stay with the durable consumer on queue pressure. No
    /// implicit ACK on delivery, clone, channel installation or scope closure.
    pub async fn complete_batch(&self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        ticket: &ServerRequestTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        let attempt = self.original(owner).map_err(|_| RuntimeError::Scope)?;
        attempt.commands(owner, scope)?.complete_batch(owner, scope, ticket, accepted).await
    }
    pub async fn complete_direct(&self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        ticket: &DirectReplyTicket, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        let attempt = self.original(owner).map_err(|_| RuntimeError::Scope)?;
        attempt.commands(owner, scope)?.complete_direct(owner, scope, ticket, accepted).await
    }
    pub async fn complete_call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallReplyTicket,accepted:bool,properties:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{
        let attempt=self.original(owner).map_err(|_|RuntimeError::Scope)?;
        attempt.commands(owner,scope)?.complete_call(owner,scope,ticket,accepted,properties).await
    }
    pub async fn call(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallControlTicket,operation:CallOperation,properties:&[RpcProperty<'_>])->Result<OwnedCallOperationReply,RuntimeError>{
        let attempt=self.original(owner).map_err(|_|RuntimeError::Scope)?;
        attempt.commands(owner,scope)?.call(owner,scope,ticket,operation,properties).await
    }
    async fn login_original(&self, owner: AttemptOwner, request: SdkLoginRequest<'_>) -> Result<(), BackendFailure> {
        let captured = LoginCapture::capture(request, &self.profile)?;
        let attempt;
        let prepared;
        let bootstrap_prepared;
        let began=Instant::now();
        {
            let mut state = lock(&self.state);
            if state.slot.as_ref().is_some_and(|a| !lock(&a.state).released)
                || state.owners.iter().any(|old| old == &owner) || state.owners.len() >= MAX_ATTEMPTS {
                return Err(BackendFailure::Unavailable);
            }
            // Entropy does not establish global u32 uniqueness. Check bounded
            // backend-local history and fail rather than ever reuse an epoch.
            let mut incarnation = None;
            for _ in 0..16 {
                let mut bytes = [0; 4]; SystemRandom::new().fill(&mut bytes).map_err(|_| BackendFailure::Unavailable)?;
                let value = u32::from_ne_bytes(bytes);
                if value != 0 && !state.incarnations.contains(&value) { incarnation = Some(value); break; }
            }
            let incarnation = incarnation.ok_or(BackendFailure::Unavailable)?;
            let (authority,root,sni)=match &self.profile.bootstrap {
                Some(bootstrap)=>(bootstrap.account_trust.authority(),bootstrap.account_trust.root_der_base64(),bootstrap.account_trust.enable_sni()),
                None=>(self.profile.authority.as_str(),captured.root.as_str(),self.profile.enable_sni),
            };
            prepared = transport_actor::prepare(owner.clone(), PacketRoute::DirectTlsCarrier,
                TlsParameters { authority, root_der_base64:root, tls_only:true, enable_sni:sni,
                    handshake_timeout:self.profile.handshake_timeout,partial_timeout:self.profile.partial_timeout },
                RuntimePolicy { io_timeout:self.profile.io_timeout,rpc_timeout:self.profile.rpc_timeout },None)
                .map_err(|_|BackendFailure::Unavailable)?;
            bootstrap_prepared=if self.profile.bootstrap.is_some(){Some(transport_actor::prepare(owner.clone(),PacketRoute::DirectTlsCarrier,
                TlsParameters{authority:&self.profile.authority,root_der_base64:&captured.root,tls_only:true,enable_sni:self.profile.enable_sni,
                    handshake_timeout:self.profile.handshake_timeout,partial_timeout:self.profile.partial_timeout},
                RuntimePolicy{io_timeout:self.profile.io_timeout,rpc_timeout:self.profile.rpc_timeout},None).map_err(|_|BackendFailure::Unavailable)?)}else{None};
            attempt = Arc::new(Attempt { owner: owner.clone(), scope: prepared.connection_scope(), incarnation,
                state: Mutex::new(AttemptState::default()), commands: Mutex::new(None), handle: AsyncMutex::new(None),
                bootstrap_scope:bootstrap_prepared.as_ref().map(|prepared|prepared.connection_scope()),bootstrap_handle:AsyncMutex::new(None),
                work: AsyncMutex::new(()), poll_work: AsyncMutex::new(()) });
            state.owners.push(owner); state.incarnations.push(incarnation); state.slot = Some(attempt.clone());
        }
        let mut retirement = Retirement::new(attempt.clone());
        let _work = attempt.work.lock().await;
        let operation=async {
        if !attempt.scope.is_open() || lock(&attempt.state).retired { return Err(BackendFailure::Disconnected); }
        let provisioned=match bootstrap_prepared {
            Some(bootstrap)=>Some(self.bootstrap_original(&attempt,&captured,bootstrap).await?),None=>None,
        };
        let input=match (&provisioned,&self.profile.bootstrap){
            (Some(plan),Some(bootstrap))=>plan.connection_for(&attempt.owner,&attempt.scope,&bootstrap.account_trust).map_err(|_|BackendFailure::Unavailable)?,
            (None,None)=>captured.connection(&self.profile),_=>return Err(BackendFailure::Unavailable),
        };
        let connected = tokio::select! {
            biased;
            _ = attempt.scope.closed() => return Err(BackendFailure::Disconnected),
            result = tokio::time::timeout(self.profile.handshake_timeout,
                self.connector.connect(&attempt.owner, &attempt.scope, input)) => {
                result.map_err(|_| BackendFailure::Disconnected)?.map_err(|_| BackendFailure::Disconnected)?
            }
        };
        let io = connected.into_original_io(&attempt.owner, &attempt.scope, input).map_err(|_| BackendFailure::Disconnected)?;
        // Acquire the owning slot BEFORE spawn/attach. Once an actor exists,
        // no fallible/await step may drop its only Join and then claim that an
        // unattached attempt was released. Cancellation here only drops the IO.
        let mut owning = attempt.handle.lock().await;
        let (handle, runtime_events) = prepared.attach(io).map_err(|_| BackendFailure::Disconnected)?;
        let commands = handle.command_handle(); *owning = Some(handle); drop(owning);
        *lock(&attempt.commands) = Some(commands.clone());
        tokio::spawn(event_pump(attempt.clone(), self.profile.clone(), self.events.clone(), runtime_events));
        let mut client = borrowed(&self.profile.client_properties);
        if self.profile.include_device_id {
            if let Some(device) = &captured.device { client.push(RpcProperty { key: b"DeviceId", value: device.as_bytes() }); }
        }
        let routing = borrowed(&self.profile.login_routing);
        // Only NotReady is retryable: actor has submitted NO login at that
        // point. Once any receipt/error other than NotReady arrives, no replay.
        let derived=provisioned.as_ref().map(|plan|plan.fields_for(&attempt.owner,&attempt.scope)).transpose().map_err(|_|BackendFailure::Unavailable)?;
        let login=||match &derived {
            Some(fields)=>ArcLoginInputs{typed_identity:captured.identity.as_bytes(),domain_name:fields.domain_name,
                selected_session_id:captured.selector.as_bytes(),phone_login_token:captured.password.as_bytes(),application_key:captured.appkey.as_bytes(),
                domain_id:fields.domain_id,application_id:fields.application_id,device_info_cc:fields.device_info_cc,client_properties:&client},
            None=>captured.login(&self.profile,&client),
        };
        let submitted = tokio::time::timeout(self.profile.handshake_timeout, async {
            loop {
                match commands.login(&attempt.owner, &attempt.scope, login(), &routing).await {
                    Err(RuntimeError::Tls(TlsError::NotReady)) => tokio::time::sleep(TLS_RETRY_CADENCE).await,
                    result => return result,
                }
            }
        }).await.map_err(|_| BackendFailure::Disconnected)?.map_err(|_| BackendFailure::Disconnected)?;
        if !submitted.submitted_on(&attempt.owner, &attempt.scope) { return Err(BackendFailure::Disconnected); }
        Ok(())
        };
        if let Some(bootstrap)=&self.profile.bootstrap {
            let deadline=began.checked_add(bootstrap.handoff_timeout).ok_or(BackendFailure::Unavailable)?;
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline),operation).await.map_err(|_|BackendFailure::Disconnected)??;
        }else{operation.await?;}
        retirement.disarm(); Ok(())
    }
    async fn bootstrap_original(&self,attempt:&Arc<Attempt>,captured:&LoginCapture,prepared:transport_actor::PreparedRuntime)->Result<ProvisionedLogin,BackendFailure>{
        let bootstrap=self.profile.bootstrap.as_ref().ok_or(BackendFailure::Unavailable)?;
        let scope=attempt.bootstrap_scope.as_ref().ok_or(BackendFailure::Unavailable)?;
        if prepared.connection_scope().matches(&attempt.scope)||!prepared.connection_scope().matches(scope){return Err(BackendFailure::Unavailable);}
        let input=captured.connection(&self.profile);
        let connected=tokio::select!{biased;
            _=scope.closed()=>return Err(BackendFailure::Disconnected),
            _=self.events.closed()=>return Err(BackendFailure::Disconnected),
            result=self.connector.connect(&attempt.owner,scope,input)=>result.map_err(|_|BackendFailure::Disconnected)?,
        };
        let io=connected.into_original_io(&attempt.owner,scope,input).map_err(|_|BackendFailure::Disconnected)?;
        let mut owning=attempt.bootstrap_handle.lock().await;
        let(handle,mut events)=prepared.attach(io).map_err(|_|BackendFailure::Disconnected)?;
        let commands=handle.command_handle();*owning=Some(handle);drop(owning);
        let routing=borrowed(&bootstrap.routing);
        let submitted=loop{
            match commands.bootstrap(&attempt.owner,scope,captured.appkey.as_bytes(),&routing).await {
                Err(RuntimeError::Tls(TlsError::NotReady))=>tokio::time::sleep(TLS_RETRY_CADENCE).await,
                Ok(receipt) if receipt.submitted_on(&attempt.owner,scope)=>break receipt,
                _=>return Err(BackendFailure::Unavailable),
            }
        };
        let call_id=submitted.call_id.ok_or(BackendFailure::Unavailable)?;
        let plan=loop{
            let event=tokio::select!{biased;
                _=self.events.closed()=>return Err(BackendFailure::Disconnected),
                event=events.recv()=>event.ok_or(BackendFailure::Unavailable)?,
            };
            if event.terminal_failure(&attempt.owner,scope).is_some(){return Err(BackendFailure::Unavailable);}
            let visible=event.into_events(&attempt.owner,scope).ok_or(BackendFailure::Disconnected)?;
            let mut reply=None;
            for event in visible {
                match event {
                    OfflineEvent::Bootstrap(value) if value.call_id()==call_id && reply.is_none()=>reply=Some(value),
                    OfflineEvent::Control(_)=>{},_=>return Err(BackendFailure::Unavailable),
                }
            }
            if let Some(reply)=reply{
                break ProvisionedLogin::from_reply(&reply,&attempt.owner,scope,&attempt.scope,&bootstrap.account_trust,
                    self.profile.reference_decode_key.as_ref(),bootstrap.client_key.as_ref()).map_err(|_|BackendFailure::Unavailable)?;
            }
        };
        if !plan.original_source_scope().matches(scope){return Err(BackendFailure::Unavailable);}
        // Immutable data is captured while source is still open. Now release
        // that owning actor before ANY destination connect. A cancelled Join
        // stays in this exact slot for logout cleanup; no active IO is replaced.
        let mut owning=attempt.bootstrap_handle.lock().await;
        let release=owning.as_mut().ok_or(BackendFailure::Disconnected)?.close().await;
        if !release.io_released(){return Err(BackendFailure::Disconnected);}
        *owning=None;
        if !attempt.scope.is_open()||lock(&attempt.state).retired{return Err(BackendFailure::Disconnected);}
        Ok(plan)
    }
    async fn status_original(&self, owner: AttemptOwner, status: i32, registration: GatewayRegistration<'_>) -> Result<(), BackendFailure> {
        if !matches!(status, STATUS_REGISTER | STATUS_UNREGISTER) { return Err(BackendFailure::Unavailable); }
        let attempt = self.original(&owner)?;
        let _work = attempt.work.lock().await;
        {
            let state = lock(&attempt.state);
            if status == STATUS_UNREGISTER && state.unregister_started {
                // Remember only the original semantic result; do not replay it
                // or treat it as a new open-connection/channel permission.
                return state.unregister_result.unwrap_or(Err(BackendFailure::Disconnected));
            }
            if state.retired || !state.configured || state.logout_started || state.unregister_started {
                return Err(BackendFailure::Disconnected);
            }
        }
        let commands = attempt.commands(&owner, &attempt.scope).map_err(|_| BackendFailure::Disconnected)?;
        let routing = borrowed(&self.profile.poll_routing); let metadata = borrowed(&self.profile.gateway_metadata);
        let mut retirement = Retirement::new(attempt.clone());
        if status == STATUS_UNREGISTER { lock(&attempt.state).unregister_started = true; }
        let result = commands.hosting(&owner, &attempt.scope, &self.profile.gateway_uri, status, registration, &metadata, &routing).await
            .and_then(|receipt| if receipt.matches(&owner, &attempt.scope) { Ok(()) } else { Err(RuntimeError::Scope) })
            .map_err(|error| if error == RuntimeError::UnregistrationRejected { BackendFailure::HostingRejected } else { BackendFailure::Disconnected });
        if status == STATUS_UNREGISTER { lock(&attempt.state).unregister_result = Some(result); }
        if result.is_ok() { retirement.disarm(); }
        result
    }
    async fn logout_original(&self, owner: AttemptOwner) -> Result<(), BackendFailure> {
        let attempt = self.original(&owner)?;
        let _work = attempt.work.lock().await;
        if lock(&attempt.state).released { return Ok(()); }
        let mut retirement = Retirement::new(attempt.clone());
        let mut bootstrap=attempt.bootstrap_handle.lock().await;
        if let Some(handle)=bootstrap.as_mut(){
            let released=handle.close().await;
            if !released.io_released(){return Err(BackendFailure::Disconnected);}
        }
        *bootstrap=None;drop(bootstrap);
        let mut owning = attempt.handle.lock().await;
        let Some(handle) = owning.as_mut() else {
            // work barrier guarantees the cancelled connector/attach future is
            // gone before an unattached attempt can be marked released.
            attempt.retire(); lock(&attempt.state).released = true; retirement.disarm(); return Ok(());
        };
        let poll_admission = attempt.poll_work.lock().await;
        let should_logout = {
            let mut state = lock(&attempt.state);
            let submit = !state.logout_started && state.configured && !state.retired && attempt.scope.is_open();
            // A cancelled wait must never replay logout on cleanup retry.
            state.logout_started = true; submit
        };
        drop(poll_admission);
        let semantic = if should_logout {
            let commands = handle.command_handle(); let routing = borrowed(&self.profile.logout_routing);
            let result = commands.logout(&owner, &attempt.scope, &routing).await.map_err(|_| BackendFailure::Disconnected);
            lock(&attempt.state).logout_result = Some(result); result
        } else { lock(&attempt.state).logout_result.unwrap_or(Ok(())) };
        attempt.retire();
        let release = handle.close().await;
        if !release.io_released() { return Err(BackendFailure::Disconnected); }
        let outcome = if should_logout && semantic.is_ok() && release.reason != RuntimeError::LoggedOut {
            Err(BackendFailure::Disconnected)
        } else { semantic };
        {
            let mut state = lock(&attempt.state); state.released = true;
            if should_logout { state.logout_result = Some(outcome); }
        }
        *owning = None; retirement.disarm();
        // First call exposes semantic rejection separately from release. A
        // controller retry confirms remembered release without any RPC replay.
        outcome
    }
}
impl HostingBackend for NativeHostingBackend {
    fn login<'a>(&'a self, owner: AttemptOwner, request: SdkLoginRequest<'a>) -> BackendFuture<'a> {
        Box::pin(self.login_original(owner, request))
    }
    fn update_status<'a>(&'a self, owner: AttemptOwner, status: i32, registration: GatewayRegistration<'a>) -> BackendFuture<'a> {
        Box::pin(self.status_original(owner, status, registration))
    }
    fn retire(&self, owner: &AttemptOwner) { if let Ok(attempt) = self.original(owner) { attempt.retire(); } }
    fn logout(&self, owner: AttemptOwner) -> BackendFuture<'_> { Box::pin(self.logout_original(owner)) }
}
impl Drop for NativeHostingBackend {
    fn drop(&mut self) { if let Some(attempt) = &lock(&self.state).slot { attempt.retire(); } }
}

fn emit(sender: &mpsc::Sender<BackendEvent>, attempt: &Attempt, payload: BackendEventPayload) -> bool {
    if sender.capacity() <= 1 { return false; }
    sender.try_send(BackendEvent { owner: attempt.owner.clone(), scope: attempt.scope.clone(), payload }).is_ok()
}
fn terminal(sender: &mpsc::Sender<BackendEvent>, attempt: &Attempt, reason: BackendFailure) {
    attempt.retire();
    // The pump is the only producer and normal events reserve this slot.
    let _ = sender.try_send(BackendEvent { owner: attempt.owner.clone(), scope: attempt.scope.clone(), payload: BackendEventPayload::Terminal(reason) });
}
async fn terminal_after_runtime_close(sender: &mpsc::Sender<BackendEvent>, attempt: &Attempt,
    events: &mut mpsc::Receiver<RuntimeEvent>, timeout: Duration) {
    // Scope close can be observed before the actor has queued its private
    // negative callback. Await only that original actor's bounded shutdown
    // channel; never reopen a message/ready path or bind a current owner.
    let original = tokio::time::timeout(timeout, async {
        while let Some(event) = events.recv().await {
            if let Some(reason) = event.terminal_failure(&attempt.owner, &attempt.scope) { return Some(reason); }
        }
        None
    }).await.ok().flatten().unwrap_or(BackendFailure::Disconnected);
    terminal(sender, attempt, original);
}
async fn event_pump(attempt: Arc<Attempt>, profile: Arc<RuntimeProfile>, sender: mpsc::Sender<BackendEvent>, mut events: mpsc::Receiver<RuntimeEvent>) {
    let mut timer = tokio::time::interval(POLL_CADENCE); timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = sender.closed() => { attempt.retire(); return; }
            _ = attempt.scope.closed() => { terminal_after_runtime_close(&sender, &attempt, &mut events, profile.io_timeout).await; return; }
            event = events.recv() => {
                let Some(event) = event else { terminal(&sender, &attempt, BackendFailure::Disconnected); return; };
                if let Some(reason) = event.terminal_failure(&attempt.owner, &attempt.scope) {
                    terminal(&sender, &attempt, reason); return;
                }
                let Some(visible) = event.into_events(&attempt.owner, &attempt.scope) else {
                    terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                };
                for event in visible {
                    let payload = match event {
                        OfflineEvent::Callback(callback) => {
                            if callback.owner != attempt.owner { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                            // Only a real, owned successful SDK callback may
                            // install scheduler state, BEFORE controller delivery.
                            if callback.callback == (Callback::SdkLogin { success: true }) {
                                let commands = match attempt.commands(&attempt.owner, &attempt.scope) {
                                    Ok(commands) => commands, Err(_) => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                                };
                                let mut retirement = Retirement::new(attempt.clone());
                                match commands.configure(&attempt.owner, &attempt.scope, attempt.incarnation, profile.incoming.as_ref().map(Binding::borrowed)).await {
                                    Ok(receipt) if receipt.matches(&attempt.owner, &attempt.scope) => {
                                        if let Some(binding)=&profile.call_incoming{
                                            match commands.configure_calls(&attempt.owner,&attempt.scope,binding.borrowed()).await{
                                                Ok(receipt)if receipt.matches(&attempt.owner,&attempt.scope)=>{},
                                                _=>{terminal(&sender,&attempt,BackendFailure::Disconnected);return;}
                                            }
                                        }
                                        if let Some(routing)=&profile.call_control{
                                            match commands.configure_call_control(&attempt.owner,&attempt.scope,&borrowed(routing)).await{
                                                Ok(receipt)if receipt.matches(&attempt.owner,&attempt.scope)=>{},
                                                _=>{terminal(&sender,&attempt,BackendFailure::Disconnected);return;}
                                            }
                                        }
                                        lock(&attempt.state).configured = true; retirement.disarm();
                                    }
                                    _ => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                                }
                            }
                            if matches!(callback.callback, Callback::Channels { .. }) {
                                terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                            }
                            BackendEventPayload::Callback(callback)
                        }
                        OfflineEvent::Message(message) => {
                            if message.owner() != &attempt.owner || !message.ticket().matches(&attempt.owner, &attempt.scope) {
                                terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                            }
                            BackendEventPayload::BatchMessage(message)
                        }
                        OfflineEvent::DirectMessage(message) => {
                            if message.owner() != &attempt.owner || !message.reply_ticket().matches(&attempt.owner, &attempt.scope) {
                                terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                            }
                            BackendEventPayload::DirectMessage(message)
                        }
                        OfflineEvent::Call(call)=>{
                            if call.owner()!=&attempt.owner||!call.ticket().matches(&attempt.owner,&attempt.scope){terminal(&sender,&attempt,BackendFailure::Disconnected);return;}
                            BackendEventPayload::Call(call)
                        }
                        // No fake interpretation of control/bootstrap/outer ACK.
                        OfflineEvent::Control(_) => continue,
                        _ => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                    };
                    if !emit(&sender, &attempt, payload) {
                        terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                    }
                }
            }
            _ = timer.tick() => {
                // Do not take control work: status3 may need subsequent polls
                // to obtain its nested result after the outer ACK retired.
                // Only serialize poll ADMISSION with starting SDK logout.
                let _poll_admission = attempt.poll_work.lock().await;
                let poll = { let state = lock(&attempt.state); state.configured && !state.retired && !state.logout_started };
                if !poll { continue; }
                let commands = match attempt.commands(&attempt.owner, &attempt.scope) {
                    Ok(commands) => commands, Err(_) => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                };
                let routing = borrowed(&profile.poll_routing); let mut retirement = Retirement::new(attempt.clone());
                match commands.poll(&attempt.owner, &attempt.scope, &routing).await {
                    Ok(receipt) if receipt.matches(&attempt.owner, &attempt.scope) => retirement.disarm(),
                    _ => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "runtime_backend_tests.rs"]
pub(crate) mod tests;
