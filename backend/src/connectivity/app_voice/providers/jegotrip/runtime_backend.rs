//! Explicit, original-attempt native driver. Not installed as a global provider.
//!
//! Original terminal authentication/hosting failures retain their private
//! actor-produced reason even when ARC has already retired the physical scope.
//! Malformed data remains a protocol failure, never a fabricated native reject.
//! A supplied profile is deployment data, never account/channel readiness.
//! DirectTlsCarrier remains supported; supplied_common additionally selects an
//! immutable physical Router policy and its exact logical ARC/SARC reference.
//! Neither constructor invents defaults or completes automatic App assembly.
//! No paid outbound, automatic consumer ACK or fabricated Channels event.
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
    runtime_provision::{AccountTrust, ProvisionedCommonLogin, ProvisionedLogin},
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

pub struct RuntimeCommonBootstrapInputs<'a>{pub routing:&'a [RpcProperty<'a>],pub client_config_decode_key:Option<ConfigDecodeKey>,pub handoff_timeout:Duration}
impl fmt::Debug for RuntimeCommonBootstrapInputs<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RuntimeCommonBootstrapInputs(<redacted>)")}}
struct CommonBootstrapProfile{routing:Vec<Property>,client_key:Option<ConfigDecodeKey>,handoff_timeout:Duration}

/// Explicit physical Router trust/access plus the original logical SDK entry.
///This is deployment data, not a handshake/authentication/hosting grant.
pub struct RuntimeCommonInputs<'a>{
    pub router:super::router_connector::RouterPolicy,
    pub account_reference:&'a str,
    pub access:&'a [RpcProperty<'a>],
    pub endpoint_index:usize,pub resolved_host_index:usize,
    pub connect_timeout:Duration,pub startup_timeout:Duration,pub attempt_timeout:Duration,
    pub compress_above:Option<usize>,
}
impl fmt::Debug for RuntimeCommonInputs<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RuntimeCommonInputs(<redacted>)")}}
struct CommonProfile{
    router:super::router_connector::RouterPolicy,entry:Option<super::object_reference::ObjectReference>,reference:Option<Zeroizing<String>>,access:Vec<Property>,
    endpoint_index:usize,resolved_host_index:usize,connect_timeout:Duration,startup_timeout:Duration,attempt_timeout:Duration,compress_above:Option<usize>,
}
impl CommonProfile{
    /// Either the original logical entry is supplied directly, or it is left
    /// unknown until a matched bootstrap returns it. Nothing is inferred from
    /// DomainName, a host string or a default endpoint in either mode.
    fn identity(&self)->Option<&[u8]>{self.entry.as_ref().map(|entry|entry.identity())}
    fn endpoint(&self)->Option<&super::object_reference::ReferenceEndpoint>{
        let super::object_reference::ReferenceTarget::Direct(endpoints)=self.entry.as_ref()?.target()else{return None};
        endpoints.get(self.endpoint_index)
    }
    fn capture(input:RuntimeCommonInputs<'_>,key:Option<&ConfigDecodeKey>,total:&mut usize,bootstrap:bool)->Result<Self,ProfileError>{
        if (!bootstrap&&input.account_reference.is_empty())||input.account_reference.len()>MAX_FIELD_BYTES||input.account_reference.chars().any(char::is_control)
            ||input.resolved_host_index>=32||input.compress_above.is_some_and(|limit|limit==0||limit>MAX_PROFILE_BYTES){return Err(ProfileError::InvalidConfiguration);}
        if bootstrap&&(input.endpoint_index!=0||input.resolved_host_index!=0){return Err(ProfileError::InvalidConfiguration);}
        // In bootstrap mode the logical entry is unknown by design. A supplied
        //reference is refused rather than silently ignored as if it were used.
        if bootstrap&&!input.account_reference.is_empty(){return Err(ProfileError::InvalidConfiguration);}
        for(timeout,max)in[(input.connect_timeout,10),(input.startup_timeout,30),(input.attempt_timeout,120)]{
            if timeout.is_zero()||timeout>Duration::from_secs(max){return Err(ProfileError::InvalidConfiguration);}
        }
        if !input.access.is_empty() && (input.access.len()!=2||![b"accessKey".as_slice(),b"accessAuth".as_slice()].iter().all(|key|input.access.iter().any(|p|p.key==*key&&!p.value.is_empty()))){return Err(ProfileError::InvalidConfiguration);}
        let(entry,reference)=if bootstrap{(None,None)}else{
            let decoded=super::legacy_config::decode_configuration(input.account_reference.as_bytes(),key).map_err(|_|ProfileError::InvalidConfiguration)?;
            let entry=super::object_reference::parse_object_reference(decoded.expose_to_backend()).map_err(|_|ProfileError::UnsupportedTransport)?;
            let super::object_reference::ReferenceTarget::Direct(endpoints)=entry.target()else{return Err(ProfileError::UnsupportedTransport);};
            if endpoints.is_empty()||endpoints.len()>16||input.endpoint_index>=endpoints.len(){return Err(ProfileError::UnsupportedTransport);}
            for endpoint in endpoints{
                if !matches!(endpoint.scheme(),b"arc"|b"sarc")||endpoint.router_host().is_err(){return Err(ProfileError::UnsupportedTransport);}
            }
            if matches!(endpoints[input.endpoint_index].router_host(),Ok(super::object_reference::RouterHost::Address(_)))&&input.resolved_host_index!=0{return Err(ProfileError::InvalidConfiguration);}
            let encoded=entry.encode_native().map_err(|_|ProfileError::InvalidConfiguration)?;
            let reference=std::str::from_utf8(encoded.expose_to_backend()).map_err(|_|ProfileError::InvalidConfiguration)?;
            add_bytes(total,reference.len()*2)?;
            (Some(entry),Some(Zeroizing::new(reference.to_owned())))
        };
        add_bytes(total,input.router.retained_bytes())?;
        let access=properties(input.access,total)?;
        Ok(Self{router:input.router,entry,reference,access,endpoint_index:input.endpoint_index,resolved_host_index:input.resolved_host_index,
            connect_timeout:input.connect_timeout,startup_timeout:input.startup_timeout,attempt_timeout:input.attempt_timeout,compress_above:input.compress_above})
    }
}

/// Bounded, owned, redacted, zeroizing deployment data. No Serialize/Default.
pub struct RuntimeProfile {
    authority: Zeroizing<String>, enable_sni: bool, identity_type: Zeroizing<String>,
    domain_name: Zeroizing<String>, domain_id: Zeroizing<String>, application_id: Zeroizing<String>,
    device_info_cc: Zeroizing<String>, client_properties: Vec<Property>, include_device_id: bool,
    login_routing: Vec<Property>, gateway_uri: Option<Zeroizing<String>>, gateway_metadata: Vec<Property>,
    poll_routing: Vec<Property>, logout_routing: Vec<Property>, incoming: Option<Binding>, offline_reception: bool, client_ex: bool,
    reference_decode_key: Option<ConfigDecodeKey>, handshake_timeout: Duration, partial_timeout: Duration,
    io_timeout: Duration, rpc_timeout: Duration, bootstrap:Option<BootstrapProfile>, common:Option<CommonProfile>, common_bootstrap:Option<CommonBootstrapProfile>, call_incoming:Option<CallBinding>,call_control:Option<Vec<Property>>, byte_length:usize,
}
impl fmt::Debug for RuntimeProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RuntimeProfile(<redacted>)") }
}
impl RuntimeProfile {
    /// 明确的本地消费策略，不发送新属性，不创造接收器或服务端认可。
    pub fn with_offline_reception(mut self, enabled: bool) -> Result<Self, ProfileError> {
        if enabled && self.incoming.is_none() { return Err(ProfileError::InvalidConfiguration); }
        self.offline_reception = enabled; Ok(self)
    }
    /// Opt in to exact ClientEx controls using the existing original receiver.
    /// This neither derives a native object/domain nor registers AccountAlive.
    pub fn with_client_ex(mut self, enabled: bool) -> Result<Self, ProfileError> {
        if enabled {
            let binding = self.incoming.as_ref().ok_or(ProfileError::InvalidConfiguration)?;
            let _registration = super::rpc_client_ex::ClientExRegistration::capture(binding.borrowed())
                .map_err(|_| ProfileError::InvalidConfiguration)?;
        }
        self.client_ex = enabled; Ok(self)
    }
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
    pub fn supplied(input: RuntimeProfileInputs<'_>) -> Result<Self, ProfileError> { Self::capture(input,None,None,None) }
    pub fn supplied_common(input:RuntimeProfileInputs<'_>,common:RuntimeCommonInputs<'_>)->Result<Self,ProfileError>{Self::capture(input,None,Some(common),None)}
    pub fn supplied_common_bootstrap(input:RuntimeProfileInputs<'_>,common:RuntimeCommonInputs<'_>,bootstrap:RuntimeCommonBootstrapInputs<'_>)->Result<Self,ProfileError>{
        if [input.domain_name,input.domain_id,input.application_id,input.device_info_cc].iter().any(|value|!value.is_empty())
            ||bootstrap.handoff_timeout.is_zero()||bootstrap.handoff_timeout>Duration::from_secs(120){return Err(ProfileError::InvalidConfiguration);}
        Self::capture(input,None,Some(common),Some(bootstrap))
    }
    /// Exact imported bootstrap TLS anchor plus separately imported account
    /// trust. Login domain/app/CC must come from the matched bootstrap, never
    /// placeholder/manual values silently substituted for returned properties.
    pub fn supplied_bootstrap(input:RuntimeProfileInputs<'_>,bootstrap:RuntimeBootstrapInputs<'_>)->Result<Self,ProfileError>{
        if [input.domain_name,input.domain_id,input.application_id,input.device_info_cc].iter().any(|value|!value.is_empty())
            ||bootstrap.handoff_timeout.is_zero()||bootstrap.handoff_timeout>Duration::from_secs(30){return Err(ProfileError::InvalidConfiguration);}
        Self::capture(input,Some(bootstrap),None,None)
    }
    fn capture(input:RuntimeProfileInputs<'_>,bootstrap:Option<RuntimeBootstrapInputs<'_>>,common:Option<RuntimeCommonInputs<'_>>,common_bootstrap:Option<RuntimeCommonBootstrapInputs<'_>>)->Result<Self,ProfileError>{
        let native=common.is_some();
        if native{
            // InnerTLS authority/SNI have no meaning on native Common. Require
            //their explicit absence; only the Router's captured anchors apply.
            if !matches!(input.route,PacketRoute::NativeCommon)||!input.authority.is_empty()||input.enable_sni||bootstrap.is_some(){return Err(ProfileError::InvalidConfiguration);}
        }else if !matches!(input.route, PacketRoute::DirectTlsCarrier) { return Err(ProfileError::UnsupportedTransport); }
        if (!native&&!valid_authority(input.authority)) || input.identity_type.is_empty()
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
        let common=common.map(|route|CommonProfile::capture(route,input.reference_decode_key.as_ref(),&mut total,common_bootstrap.is_some())).transpose()?;
        if let Some(common)=&common{
            // Original reference identity is not replaceable through a profile
            //metadata map. Fill an absent id below; reject any conflicting one.
            //Before a matched bootstrap the identity is still unknown, so a
            //profile-carried id cannot be honoured or guessed either.
            for fields in [input.login_routing,input.poll_routing,input.logout_routing]{
                if common_bootstrap.is_some(){if fields.iter().any(|p|p.key==b"id"){return Err(ProfileError::InvalidConfiguration);}}
                else if let Some(identity)=common.identity(){if fields.iter().any(|p|p.key==b"id"&&p.value!=identity){return Err(ProfileError::InvalidConfiguration);}}
            }
        }
        let from_bootstrap=bootstrap.is_some()||common_bootstrap.is_some();
        let common_bootstrap=common_bootstrap.map(|bootstrap|->Result<CommonBootstrapProfile,ProfileError>{
            let native=common.as_ref().ok_or(ProfileError::InvalidConfiguration)?;
            if native.identity().is_some(){return Err(ProfileError::InvalidConfiguration);}
            if bootstrap.routing.iter().any(|p|p.key==b"id"){return Err(ProfileError::InvalidConfiguration);}
            Ok(CommonBootstrapProfile{routing:routing(bootstrap.routing,&mut total)?,client_key:bootstrap.client_config_decode_key,handoff_timeout:bootstrap.handoff_timeout})
        }).transpose()?;
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
        let account_routing=|fields:&[RpcProperty<'_>],total:&mut usize|->Result<Vec<Property>,ProfileError>{
            let mut result=routing(fields,total)?;
            if let Some(common)=&common{if let(Some(identity),None)=(common.identity(),&common_bootstrap){if !fields.iter().any(|p|p.key==b"id"){
                if result.len()>=MAX_MAP_ENTRIES{return Err(ProfileError::ResourceLimit);}add_bytes(total,2+identity.len())?;
                result.push(Property{key:b"id".to_vec(),value:identity.to_vec()});
            }}}Ok(result)
        };
        let login_routing=account_routing(input.login_routing,&mut total)?;let poll_routing=account_routing(input.poll_routing,&mut total)?;let logout_routing=account_routing(input.logout_routing,&mut total)?;
        // Real `current_user_uri` (Mtc_UeGetUri) is the normalized loginSession
        //identity. An empty supplied URI therefore means "derive it from the
        //matched session" instead of requiring the user to retype it; only the
        //native bootstrap route may request that derivation.
        let derived_uri=native&&common_bootstrap.is_some()&&input.gateway_uri.is_empty();
        // Hosting registers against the SipGateway object. When the native
        //profile does not name it, use the original default object id instead of
        //sending the Account object or a placeholder.
        let mut gateway_metadata=routing(input.gateway_metadata, &mut total)?;
        if native&&!gateway_metadata.iter().any(|p|p.key==b"id"){
            if gateway_metadata.len()>=MAX_MAP_ENTRIES{return Err(ProfileError::ResourceLimit);}
            let gateway:&[u8]=super::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes();
            add_bytes(&mut total,2+gateway.len())?;
            gateway_metadata.push(Property{key:b"id".to_vec(),value:gateway.to_vec()});
        }
        Ok(Self {
            authority: text(input.authority, native, &mut total)?, enable_sni: input.enable_sni,
            identity_type: text(input.identity_type, false, &mut total)?,
            domain_name: text(input.domain_name, from_bootstrap, &mut total)?, domain_id: text(input.domain_id, from_bootstrap, &mut total)?,
            application_id: text(input.application_id, from_bootstrap, &mut total)?, device_info_cc: text(input.device_info_cc, true, &mut total)?,
            client_properties: properties(input.client_properties, &mut total)?, include_device_id: input.include_device_id,
            login_routing, gateway_uri: if derived_uri{None}else{Some(text(input.gateway_uri, false, &mut total)?)},
            gateway_metadata,
            poll_routing, logout_routing, incoming, offline_reception: false, client_ex: false, reference_decode_key: input.reference_decode_key,
            handshake_timeout: input.handshake_timeout, partial_timeout: input.partial_timeout,
            io_timeout: input.io_timeout, rpc_timeout: input.rpc_timeout, bootstrap, common, common_bootstrap, call_incoming:None,call_control:None,byte_length:total,
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
        let reference=reference_text(request.server_address,&mut total).map_err(invalid)?;
        if let Some(common)=&profile.common{
            if let Some(expected)=&common.reference{
                let decoded=super::legacy_config::decode_configuration(reference.as_bytes(),profile.reference_decode_key.as_ref()).map_err(|_|BackendFailure::Unavailable)?;
                let parsed=super::object_reference::parse_object_reference(decoded.expose_to_backend()).map_err(|_|BackendFailure::Unavailable)?;
                let encoded=parsed.encode_native().map_err(|_|BackendFailure::Unavailable)?;
                if encoded.expose_to_backend()!=expected.as_bytes(){return Err(BackendFailure::Unavailable);}
            }
        }
        Ok(Self { identity: typed,
            password: text(request.phone_login_token, false, &mut total).map_err(invalid)?,
            appkey: text(request.application_key, false, &mut total).map_err(invalid)?,
            reference,
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

/// Normalized loginSession identity of the submitted attempt, i.e. the SDK's
/// `Mtc_UeGetUri` handed to `updateStatus`. Built with the SAME helper the
/// login arguments use, so the URI cannot drift from the session it names.
fn session_uri(captured:&LoginCapture,domain:&[u8])->Result<Zeroizing<String>,BackendFailure>{
    let normalized=super::rpc_login::normalize_session_identity(captured.identity.as_bytes(),domain)
        .map_err(|_|BackendFailure::Unavailable)?;
    let text=std::str::from_utf8(normalized.expose_to_backend()).map_err(|_|BackendFailure::Unavailable)?;
    Ok(Zeroizing::new(text.to_owned()))
}

/// Owned events retain ORIGINAL correlation even after terminal retirement.
/// A copied callback/message is not a channel or an open-scope lease.
pub struct BackendEvent { owner: AttemptOwner, scope: RpcConnectionScope, pub payload: BackendEventPayload }
pub enum BackendEventPayload {
    Callback(OwnedRpcCallback),
    BatchMessage(ScheduledIncomingMessage),
    DirectMessage(IncomingProtocolMessage),
    OfflineHint(super::rpc_offline::IncomingOfflineHint),
    OfflinePage(super::rpc_offline::OwnedOfflinePage),
    BatchRetirement(super::batch_scheduler::BatchReplayRetirement),
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
    /// Matched bootstrap observation only; never an armed keepalive or grant.
    account_alive:Option<super::runtime_provision::AccountAliveState>,
    #[cfg(test)]
    polls_paused: bool,
}
struct Attempt {
    owner: AttemptOwner, scope: RpcConnectionScope, incarnation: u32,
    state: Mutex<AttemptState>, commands: Mutex<Option<RuntimeCommands>>,
    // Commands do not own IO. The owning handle is NEVER moved into a cleanup
    // future: cancellation of close retains its original Join in this slot.
    handle: AsyncMutex<Option<RuntimeHandle>>,
    // Normalized loginSession identity of THIS attempt, i.e. the SDK's
    // current_user_uri (Mtc_UeGetUri) used by updateStatus. Captured at login
    // submission, never inferred from DomainName/host strings later.
    session_uri: Mutex<Option<Zeroizing<String>>>,
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
        *lock(&self.session_uri)=None;
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
    /// Classification survives the original matched handoff; no references or
    /// tokens are exported and this never means alive registration succeeded.
    pub(super) fn account_alive_observation(&self,owner:&AttemptOwner)->Option<&'static str> {
        let attempt=self.original(owner).ok()?;
        let observation=lock(&attempt.state).account_alive;
        observation.map(super::runtime_provision::AccountAliveState::label)
    }
    pub(super) fn original_incoming_binding(&self, owner: &AttemptOwner, scope: &RpcConnectionScope)
        -> Option<super::rpc_incoming::IncomingBinding<'_>> {
        let attempt = self.original(owner).ok()?;
        if !attempt.scope.matches(scope) || !scope.is_open() || scope.session_revoked() || !lock(&attempt.state).configured { return None; }
        self.profile.incoming.as_ref().map(Binding::borrowed)
    }
    pub(super) fn original_call_receiver_ready(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        let Ok(attempt) = self.original(owner) else { return false; };
        attempt.scope.matches(scope) && scope.is_open() && !scope.session_revoked() && lock(&attempt.state).configured
            && self.profile.call_incoming.is_some() && self.profile.call_control.is_some()
    }
    #[cfg(test)]
    pub(super) fn pause_original_polls_for_test(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, paused: bool) -> Result<(), RuntimeError> {
        // Scheduling barrier only: the actual actor, original callback/receipt
        // and all RPC admission checks still execute unchanged in the fixture.
        let attempt = self.original(owner).map_err(|_| RuntimeError::Scope)?;
        if !attempt.scope.matches(scope) || !scope.is_open() { return Err(RuntimeError::Scope); }
        lock(&attempt.state).polls_paused = paused;
        Ok(())
    }
    fn original(&self, owner: &AttemptOwner) -> Result<Arc<Attempt>, BackendFailure> {
        lock(&self.state).slot.as_ref().filter(|a| &a.owner == owner).cloned().ok_or(BackendFailure::Disconnected)
    }
    fn preserve_revocation(&self, owner: &AttemptOwner, result: Result<(), BackendFailure>) -> Result<(), BackendFailure> {
        // Ok is historical submission, never a positive SDK/hosting result.
        // Preserve its correlation while the owning actor may finish a control
        // reply; only failures must be reclassified as nonretryable revocation.
        if result.is_err() && self.original(owner).is_ok_and(|attempt| attempt.scope.session_revoked()) { Err(BackendFailure::SessionRevoked) } else { result }
    }
    async fn arm_original_client_ex(&self, attempt: &Attempt, commands: &RuntimeCommands) -> Result<(), BackendFailure> {
        if !self.profile.client_ex { return Ok(()); }
        let binding = self.profile.incoming.as_ref().ok_or(BackendFailure::Unavailable)?;
        let receipt = commands.arm_client_ex(&attempt.owner, &attempt.scope, binding.borrowed()).await
            .map_err(|_| BackendFailure::Disconnected)?;
        if !receipt.matches(&attempt.owner, &attempt.scope) { return Err(BackendFailure::Disconnected); }
        Ok(())
    }
    /// Borrowed tickets stay with the durable consumer on queue pressure. No
    /// implicit ACK on delivery, clone, channel installation or scope closure.
    pub(super) fn offline_reception_enabled(&self) -> bool { self.profile.offline_reception }
    pub(super) async fn get_offline(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, position: i64) -> Result<RuntimeReceipt, RuntimeError> {
        let attempt = self.original(owner).map_err(|_| RuntimeError::Scope)?;
        let _admission = attempt.poll_work.lock().await;
        {
            let state = lock(&attempt.state);
            if !state.configured || state.retired || state.logout_started || state.unregister_started || self.profile.incoming.is_none() || !self.profile.offline_reception {
                return Err(RuntimeError::Closed);
            }
        }
        attempt.commands(owner, scope)?.get_offline(owner, scope, position, &borrowed(&self.profile.poll_routing)).await
    }
    pub(super) async fn complete_offline_hint(&self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        hint: &super::rpc_offline::IncomingOfflineHint, accepted: bool) -> Result<RuntimeReceipt, RuntimeError> {
        let attempt = self.original(owner).map_err(|_| RuntimeError::Scope)?;
        attempt.commands(owner, scope)?.complete_offline_hint(owner, scope, hint, accepted).await
    }

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
    fn login_common_original<'a>(&'a self, owner: AttemptOwner, request: SdkLoginRequest<'a>) -> BackendFuture<'a> {
        // Native/bootstrap IO state is boxed at its entrypoint. The original
        // direct/native dispatch contract and every deadline remain unchanged.
        Box::pin(self.login_common_state(owner,request))
    }
    async fn login_common_state(&self,owner:AttemptOwner,request:SdkLoginRequest<'_>)->Result<(),BackendFailure>{
        let captured=LoginCapture::capture(request,&self.profile)?;let common=self.profile.common.as_ref().ok_or(BackendFailure::Unavailable)?;
        let begin=Instant::now();let end=begin.checked_add(common.attempt_timeout).ok_or(BackendFailure::Unavailable)?;
        // The policy is a plain value: build one per prepared Common runtime.
        let runtime_policy=||RuntimePolicy{io_timeout:self.profile.io_timeout,rpc_timeout:self.profile.rpc_timeout};
        let destination=transport_actor::native::prepare_common(owner.clone(),runtime_policy(),end).map_err(|_|BackendFailure::Unavailable)?;
        // A matched bootstrap always uses its OWN Common incarnation. That
        //actor is released before the destination dials; its returned data
        //cannot replace Router hosts/SNI/roots/access or the destination scope.
        let bootstrap=match self.profile.common_bootstrap.is_some(){
            true=>Some(transport_actor::native::prepare_common(owner.clone(),runtime_policy(),end).map_err(|_|BackendFailure::Unavailable)?),false=>None};
        if bootstrap.as_ref().is_some_and(|bootstrap|bootstrap.connection_scope().matches(&destination.connection_scope())){return Err(BackendFailure::Unavailable);}
        let bootstrap_scope=bootstrap.as_ref().map(|bootstrap|bootstrap.connection_scope());
        let attempt={
            let mut state=lock(&self.state);
            if state.slot.as_ref().is_some_and(|a|!lock(&a.state).released)||state.owners.iter().any(|old|old==&owner)||state.owners.len()>=MAX_ATTEMPTS{return Err(BackendFailure::Unavailable);}
            let mut incarnation=None;
            for _ in 0..16{let mut bytes=[0;4];SystemRandom::new().fill(&mut bytes).map_err(|_|BackendFailure::Unavailable)?;let value=u32::from_ne_bytes(bytes);
                if value!=0&&!state.incarnations.contains(&value){incarnation=Some(value);break;}}
            let incarnation=incarnation.ok_or(BackendFailure::Unavailable)?;
            let attempt=Arc::new(Attempt{owner:owner.clone(),scope:destination.connection_scope(),incarnation,state:Mutex::new(AttemptState::default()),commands:Mutex::new(None),
                handle:AsyncMutex::new(None),session_uri:Mutex::new(None),bootstrap_scope,bootstrap_handle:AsyncMutex::new(None),work:AsyncMutex::new(()),poll_work:AsyncMutex::new(())});
            state.owners.push(owner);state.incarnations.push(incarnation);state.slot=Some(attempt.clone());attempt
        };
        let mut retirement=Retirement::new(attempt.clone());let _work=attempt.work.lock().await;
        let operation=async{
            if !attempt.scope.is_open()||lock(&attempt.state).retired{return Err(BackendFailure::Disconnected);}
            let plan=match bootstrap{Some(bootstrap)=>Some(self.bootstrap_common_original(&attempt,&captured,bootstrap).await?),None=>None};
            if let Some(plan)=&plan {lock(&attempt.state).account_alive=Some(plan.account_alive());}
            // Only a matched bootstrap may supply the logical entry. Directly
            //supplied Common profiles keep their own single captured entry.
            let(resolved,entry,endpoint)=match &plan{
                Some(plan)=>{let entry=plan.entry_for(&attempt.owner,&attempt.scope,&common.router).map_err(|_|BackendFailure::Unavailable)?;
                    let super::object_reference::ReferenceTarget::Direct(endpoints)=entry.target()else{return Err(BackendFailure::Unavailable);};
                    let endpoint=endpoints.first().ok_or(BackendFailure::Unavailable)?;(0,entry,endpoint)}
                None=>(common.resolved_host_index,common.entry.as_ref().ok_or(BackendFailure::Unavailable)?,
                    common.endpoint().ok_or(BackendFailure::Unavailable)?),
            };
            let access=borrowed(&common.access);
            let connected=destination.connect(&common.router,&access,entry.identity(),endpoint,resolved,super::router_runtime::RouterTimeouts{
                connect:common.connect_timeout,handshake:self.profile.handshake_timeout,partial:self.profile.partial_timeout,io:self.profile.io_timeout,startup:common.startup_timeout},common.compress_above)
                .await.map_err(|_|BackendFailure::Disconnected)?;
            if !connected.connection_scope().matches(&attempt.scope){return Err(BackendFailure::Disconnected);}
            // No detached actor exists before this slot is held. Attaching and
            //storing its owning handle contain no await/cancellation gap.
            let mut owning=attempt.handle.lock().await;
            if !attempt.scope.is_open()||lock(&attempt.state).retired{return Err(BackendFailure::Disconnected);}
            let(handle,runtime_events)=connected.attach().map_err(|_|BackendFailure::Disconnected)?;
            let commands=handle.command_handle();*owning=Some(handle);drop(owning);*lock(&attempt.commands)=Some(commands.clone());
            tokio::spawn(event_pump(attempt.clone(),self.profile.clone(),self.events.clone(),runtime_events));
            let mut client=borrowed(&self.profile.client_properties);
            if self.profile.include_device_id{if let Some(device)=&captured.device{client.push(RpcProperty{key:b"DeviceId",value:device.as_bytes()});}}
            let routing=borrowed(&self.profile.login_routing);
            let derived=plan.as_ref().map(|plan|plan.fields_for(&attempt.owner,&attempt.scope,&common.router)).transpose().map_err(|_|BackendFailure::Unavailable)?;
            if self.profile.gateway_uri.is_none(){
                let domain=match &derived{Some(fields)=>fields.domain_name,None=>self.profile.domain_name.as_bytes()};
                *lock(&attempt.session_uri)=Some(session_uri(&captured,domain)?);
            }
            let login=||match &derived{
                Some(fields)=>ArcLoginInputs{typed_identity:captured.identity.as_bytes(),domain_name:fields.domain_name,
                    selected_session_id:captured.selector.as_bytes(),phone_login_token:captured.password.as_bytes(),application_key:captured.appkey.as_bytes(),
                    domain_id:fields.domain_id,application_id:fields.application_id,device_info_cc:fields.device_info_cc,client_properties:&client},
                None=>captured.login(&self.profile,&client)};
            self.arm_original_client_ex(&attempt, &commands).await?;
            let submitted=commands.login(&attempt.owner,&attempt.scope,login(),&routing).await.map_err(|_|BackendFailure::Disconnected)?;
            if !submitted.submitted_on(&attempt.owner,&attempt.scope){return Err(BackendFailure::Disconnected);}Ok(())
        };
        let deadline=match &self.profile.common_bootstrap{Some(bootstrap)=>begin.checked_add(bootstrap.handoff_timeout).ok_or(BackendFailure::Unavailable)?.min(end),None=>end};
        tokio::time::timeout_at(deadline.into(),operation).await.map_err(|_|BackendFailure::Disconnected)??;
        retirement.disarm();Ok(())
    }
    /// One matched bootstrap over the ORIGINAL Common/Router transport. Only
    ///decoded data leaves this scoped actor; the returned DomainId/AppId/CC are
    ///login arguments, never a new Router policy, root, or hosting grant.
    async fn bootstrap_common_original(&self,attempt:&Arc<Attempt>,captured:&LoginCapture,
        prepared:transport_actor::native::PreparedCommonRuntime)->Result<ProvisionedCommonLogin,BackendFailure>{
        let profile=self.profile.as_ref();let bootstrap=profile.common_bootstrap.as_ref().ok_or(BackendFailure::Unavailable)?;
        let common=profile.common.as_ref().ok_or(BackendFailure::Unavailable)?;
        let scope=attempt.bootstrap_scope.as_ref().ok_or(BackendFailure::Unavailable)?;
        if prepared.connection_scope().matches(&attempt.scope)||!prepared.connection_scope().matches(scope){return Err(BackendFailure::Unavailable);}
        // The SDK reference only names the ORIGINAL bootstrap entry. Its TLS
        //authority/root never replaces the explicit physical Router trust.
        let decoded=super::legacy_config::decode_configuration(captured.reference.as_bytes(),profile.reference_decode_key.as_ref()).map_err(|_|BackendFailure::Unavailable)?;
        let entry=super::object_reference::parse_object_reference(decoded.expose_to_backend()).map_err(|_|BackendFailure::Unavailable)?;
        let super::object_reference::ReferenceTarget::Direct(endpoints)=entry.target()else{return Err(BackendFailure::Unavailable);};
        if endpoints.is_empty()||endpoints.len()>16||endpoints.iter().any(|endpoint|!matches!(endpoint.scheme(),b"arc"|b"sarc")||endpoint.router_host().is_err()){
            return Err(BackendFailure::Unavailable);
        }
        let access=borrowed(&common.access);
        let connected=prepared.connect(&common.router,&access,entry.identity(),&endpoints[0],0,super::router_runtime::RouterTimeouts{
            connect:common.connect_timeout,handshake:profile.handshake_timeout,partial:profile.partial_timeout,io:profile.io_timeout,startup:common.startup_timeout},common.compress_above)
            .await.map_err(|_|BackendFailure::Disconnected)?;
        if !connected.connection_scope().matches(scope){return Err(BackendFailure::Disconnected);}
        let mut owning=attempt.bootstrap_handle.lock().await;
        if !scope.is_open()||lock(&attempt.state).retired{return Err(BackendFailure::Disconnected);}
        let(handle,mut events)=connected.attach().map_err(|_|BackendFailure::Disconnected)?;
        let commands=handle.command_handle();*owning=Some(handle);drop(owning);
        let routing=borrowed(&bootstrap.routing);
        let submitted=loop{
            match commands.bootstrap(&attempt.owner,scope,captured.appkey.as_bytes(),&routing).await{
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
            for event in visible{match event{
                OfflineEvent::Bootstrap(value) if value.call_id()==call_id&&reply.is_none()=>reply=Some(value),
                OfflineEvent::Control(_)=>{},_=>return Err(BackendFailure::Unavailable),
            }}
            if let Some(reply)=reply{
                break ProvisionedCommonLogin::from_reply(&reply,&attempt.owner,scope,&attempt.scope,&common.router,
                    profile.reference_decode_key.as_ref(),bootstrap.client_key.as_ref()).map_err(|_|BackendFailure::Unavailable)?;
            }
        };
        if !plan.original_source_scope().matches(scope){return Err(BackendFailure::Unavailable);}
        // Immutable data is captured while the source is still open. Release
        //that owning actor before ANY destination connect; a cancelled Join
        //stays in this exact slot for logout cleanup instead of being replaced.
        let mut owning=attempt.bootstrap_handle.lock().await;
        let release=owning.as_mut().ok_or(BackendFailure::Disconnected)?.close().await;
        if !release.io_released(){return Err(BackendFailure::Disconnected);}
        *owning=None;
        if !attempt.scope.is_open()||lock(&attempt.state).retired{return Err(BackendFailure::Disconnected);}
        Ok(plan)
    }
    async fn login_original(&self, owner: AttemptOwner, request: SdkLoginRequest<'_>) -> Result<(), BackendFailure> {
        if self.profile.common.is_some(){return self.login_common_original(owner,request).await;}
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
                state: Mutex::new(AttemptState::default()), commands: Mutex::new(None), handle: AsyncMutex::new(None),session_uri:Mutex::new(None),
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
        if let Some(plan)=&provisioned {lock(&attempt.state).account_alive=Some(plan.account_alive());}
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
        if self.profile.gateway_uri.is_none(){
            let domain=match &derived{Some(fields)=>fields.domain_name,None=>self.profile.domain_name.as_bytes()};
            *lock(&attempt.session_uri)=Some(session_uri(&captured,domain)?);
        }
        let login=||match &derived {
            Some(fields)=>ArcLoginInputs{typed_identity:captured.identity.as_bytes(),domain_name:fields.domain_name,
                selected_session_id:captured.selector.as_bytes(),phone_login_token:captured.password.as_bytes(),application_key:captured.appkey.as_bytes(),
                domain_id:fields.domain_id,application_id:fields.application_id,device_info_cc:fields.device_info_cc,client_properties:&client},
            None=>captured.login(&self.profile,&client),
        };
        self.arm_original_client_ex(&attempt, &commands).await?;
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
        let uri=match &self.profile.gateway_uri{
            Some(uri)=>uri.clone(),
            // No derived session URI may be fabricated: hosting needs the URI of
            //THIS authenticated attempt, never a DomainName/host inference.
            None=>lock(&attempt.session_uri).clone().ok_or(BackendFailure::Unavailable)?,
        };
        let result = commands.hosting(&owner, &attempt.scope, uri.as_str(), status, registration, &metadata, &routing).await
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
        // Construct the large original future separately on the heap; the
        // terminal-error wrapper owns only this pointer and the original owner.
        // Boxing changes layout, not admission, cancellation or IO deadlines.
        let original: BackendFuture<'a> = Box::pin(self.login_original(owner.clone(), request));
        Box::pin(async move { self.preserve_revocation(&owner, original.await) })
    }
    fn update_status<'a>(&'a self, owner: AttemptOwner, status: i32, registration: GatewayRegistration<'a>) -> BackendFuture<'a> {
        let original: BackendFuture<'a> = Box::pin(self.status_original(owner.clone(), status, registration));
        Box::pin(async move { self.preserve_revocation(&owner, original.await) })
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
    let reason = if attempt.scope.session_revoked() { BackendFailure::SessionRevoked } else { reason };
    attempt.retire();
    // The pump is the only producer and normal events reserve this slot.
    let _ = sender.try_send(BackendEvent { owner: attempt.owner.clone(), scope: attempt.scope.clone(), payload: BackendEventPayload::Terminal(reason) });
}
async fn terminal_after_runtime_close(sender: &mpsc::Sender<BackendEvent>, attempt: &Attempt,
    events: &mut mpsc::Receiver<RuntimeEvent>, timeout: Duration) {
    if attempt.scope.session_revoked() && attempt.scope.is_open() {
        // Suppress prior positive events, but do not preempt the actor's one
        // original control reply. The actor's own fixed IO/RPC deadlines still
        // apply; this bounded wait cannot reopen or extend them.
        let _ = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! { biased;
                    _ = attempt.scope.closed() => break,
                    event = events.recv() => if event.is_none() { break; },
                }
            }
        }).await;
        terminal(sender, attempt, BackendFailure::SessionRevoked); return;
    }
    // Scope close can be observed before the actor has queued its private
    // negative callback. Await only that original actor's bounded shutdown
    // channel; never reopen a message/ready path or bind a current owner.
    let original = tokio::time::timeout(timeout, async {
        while let Some(event) = events.recv().await {
            if let Some(reason) = event.terminal_failure(&attempt.owner, &attempt.scope) { return Some(reason); }
        }
        None
    }).await.ok().flatten().unwrap_or_else(|| if attempt.scope.session_revoked() { BackendFailure::SessionRevoked } else { BackendFailure::Disconnected });
    terminal(sender, attempt, original);
}
/// Coalesced control wake uses the same original poll admission, routing and
/// retirement guard as the periodic pump. It never refreshes RPC/TLS deadlines.
async fn poll_for_client_ex(attempt: &Arc<Attempt>, profile: &RuntimeProfile) -> Result<bool, BackendFailure> {
    let _admission = attempt.poll_work.lock().await;
    let poll = {
        let state = lock(&attempt.state);
        #[cfg(test)]
        if state.polls_paused { return Ok(false); }
        state.configured && !state.retired && !state.logout_started
    };
    if !poll { return Ok(false); }
    let commands = attempt.commands(&attempt.owner, &attempt.scope).map_err(|_| BackendFailure::Disconnected)?;
    let mut retirement = Retirement::new(attempt.clone());
    let receipt = commands.poll(&attempt.owner, &attempt.scope, &borrowed(&profile.poll_routing)).await
        .map_err(|_| BackendFailure::Disconnected)?;
    if !receipt.matches(&attempt.owner, &attempt.scope) { return Err(BackendFailure::Disconnected); }
    retirement.disarm(); Ok(true)
}
async fn event_pump(attempt: Arc<Attempt>, profile: Arc<RuntimeProfile>, sender: mpsc::Sender<BackendEvent>, mut events: mpsc::Receiver<RuntimeEvent>) {
    let mut timer = tokio::time::interval(POLL_CADENCE); timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut poll_requested = false;
    let mut last_poll: Option<Instant> = None;
    loop {
        // One bit coalesces hints. A flood cannot postpone a due poll by keeping
        // the receive branch ready; nor can hints reset any operation budget.
        if poll_requested && attempt.scope.is_open() && !attempt.scope.session_revoked()
            && last_poll.is_none_or(|last| last.elapsed() >= POLL_CADENCE) {
            poll_requested = false;
            match poll_for_client_ex(&attempt, &profile).await {
                Ok(true) => last_poll = Some(Instant::now()),
                Ok(false) => {},
                Err(reason) => { terminal(&sender, &attempt, reason); return; }
            }
        }
        tokio::select! {
            biased;
            _ = sender.closed() => { attempt.retire(); return; }
            _ = attempt.scope.closed() => { terminal_after_runtime_close(&sender, &attempt, &mut events, profile.io_timeout).await; return; }
            event = events.recv() => {
                let Some(event) = event else { terminal(&sender, &attempt, BackendFailure::Disconnected); return; };
                if let Some(reason) = event.terminal_failure(&attempt.owner, &attempt.scope) {
                    if reason == BackendFailure::SessionRevoked && attempt.scope.is_open() {
                        terminal_after_runtime_close(&sender, &attempt, &mut events, profile.io_timeout).await;
                    } else { terminal(&sender, &attempt, reason); }
                    return;
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
                                        if profile.client_ex {
                                            match commands.configure_client_ex(&attempt.owner, &attempt.scope).await {
                                                Ok(receipt) if receipt.matches(&attempt.owner, &attempt.scope) => {},
                                                _ => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                                            }
                                        }
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
                        OfflineEvent::ClientEx(notice) => {
                            if !notice.matches(&attempt.owner, &attempt.scope) { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                            if matches!(notice.control(), super::rpc_client_ex::ClientExControl::PollHint { .. }) { poll_requested = true; }
                            else { terminal(&sender, &attempt, BackendFailure::SessionRevoked); return; }
                            continue;
                        }
                        OfflineEvent::OfflineHint(hint) => {
                            if !hint.matches(&attempt.owner, &attempt.scope) { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                            BackendEventPayload::OfflineHint(hint)
                        }
                        OfflineEvent::OfflinePage(page) => {
                            if !page.matches(&attempt.owner, &attempt.scope) { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                            BackendEventPayload::OfflinePage(page)
                        }
                        OfflineEvent::Message(message) => {
                            if message.owner() != &attempt.owner || !message.ticket().matches(&attempt.owner, &attempt.scope) {
                                terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                            }
                            BackendEventPayload::BatchMessage(message)
                        }
                        OfflineEvent::BatchRetirement(retirement) => {
                            if !retirement.matches(&attempt.owner, &attempt.scope) {
                                terminal(&sender, &attempt, BackendFailure::Disconnected); return;
                            }
                            BackendEventPayload::BatchRetirement(retirement)
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
                let poll = {
                    let state = lock(&attempt.state);
                    #[cfg(test)]
                    if state.polls_paused { continue; }
                    state.configured && !state.retired && !state.logout_started
                };
                if !poll { continue; }
                let commands = match attempt.commands(&attempt.owner, &attempt.scope) {
                    Ok(commands) => commands, Err(_) => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                };
                let routing = borrowed(&profile.poll_routing); let mut retirement = Retirement::new(attempt.clone());
                match commands.poll(&attempt.owner, &attempt.scope, &routing).await {
                    Ok(receipt) if receipt.matches(&attempt.owner, &attempt.scope) => {
                        last_poll = Some(Instant::now()); poll_requested = false; retirement.disarm();
                    }
                    _ => { terminal(&sender, &attempt, BackendFailure::Disconnected); return; }
                }
            }
        }
    }
}

#[cfg(test)]
#[path="runtime_common_backend_tests.rs"]
mod common_tests;

#[cfg(test)]
#[path = "runtime_backend_tests.rs"]
pub(crate) mod tests;
