//! Original CallEx Accept -> separately bound native Media Endpoint session.
//! Neither a parsed map nor a true CallEx result alone grants an audio socket.
//! Direct references use an explicitly imported, independent TLS anchor. The
//! original call, account scope, media scope, trust and RPC sequence are retained.
use super::{
    call_control::{CallControlTicket,OwnedCallOperationReply,validate_routing},
    legacy_config::{ConfigDecodeKey,decode_configuration}, object_reference::parse_object_reference,
    rpc_calls::CallOperation,rpc_envelope::{decode_reply,encode_request},
    rpc_endpoint::{EndpointConnectInputs,decode_endpoint_connect_result,encode_endpoint_connect,CONNECT_ENDPOINT_METHOD},
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcBytes,RpcProperty,MAX_STREAM_BYTES},
    runtime_connector::{ConnectionInput,validate_explicit_profile}, runtime_provision::AccountTrust,
    session::AttemptOwner,
};
use std::{fmt,sync::Arc,time::Duration};
use zeroize::Zeroizing;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum MediaError{Scope,Configuration,UnsupportedReference,MissingProperty,Protocol,Rejected,Timeout,Io,Closed,Backpressure,AlreadyStarted}
impl MediaError{pub fn code(self)->&'static str{match self{
    Self::Scope=>"jegotrip_media_scope_invalid",Self::Configuration=>"jegotrip_media_configuration_invalid",
    Self::UnsupportedReference=>"jegotrip_media_reference_unsupported",Self::MissingProperty=>"jegotrip_media_property_missing",
    Self::Protocol=>"jegotrip_media_protocol_invalid",Self::Rejected=>"jegotrip_media_endpoint_rejected",
    Self::Timeout=>"jegotrip_media_deadline_expired",Self::Io=>"jegotrip_media_io_failed",
    Self::Closed=>"jegotrip_media_closed",Self::Backpressure=>"jegotrip_media_backpressure",Self::AlreadyStarted=>"jegotrip_media_already_started",
}}}
struct Property{key:Zeroizing<Vec<u8>>,value:Zeroizing<Vec<u8>>}
fn properties(input:&[RpcProperty<'_>])->Result<Vec<Property>,MediaError>{
    // The codec checks native map/set/stream bounds before ownership transfer.
    let mut writer=super::rpc_stream::Writer::new();writer.properties(input).map_err(|_|MediaError::Configuration)?;
    Ok(input.iter().map(|p|Property{key:Zeroizing::new(p.key.to_vec()),value:Zeroizing::new(p.value.to_vec())}).collect())
}
fn borrowed(input:&[Property])->Vec<RpcProperty<'_>>{input.iter().map(|p|RpcProperty{key:&p.key,value:&p.value}).collect()}

pub struct MediaEndpointProfileInputs<'a>{
    pub trust:Arc<AccountTrust>,pub routing:&'a[RpcProperty<'a>],
    pub features:&'a[&'a[u8]],pub properties:&'a[RpcProperty<'a>],
    pub reference_decode_key:Option<ConfigDecodeKey>,
    pub handshake_timeout:Duration,pub partial_timeout:Duration,pub io_timeout:Duration,pub rpc_timeout:Duration,
}
impl fmt::Debug for MediaEndpointProfileInputs<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("MediaEndpointProfileInputs(<redacted>)")}}
pub struct MediaEndpointProfile{
    trust:Arc<AccountTrust>,routing:Vec<Property>,features:Vec<Zeroizing<Vec<u8>>>,properties:Vec<Property>,
    key:Option<ConfigDecodeKey>,pub(super) handshake_timeout:Duration,pub(super) partial_timeout:Duration,
    pub(super) io_timeout:Duration,pub(super) rpc_timeout:Duration,
}
impl fmt::Debug for MediaEndpointProfile{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("MediaEndpointProfile(<redacted>)")}}
impl MediaEndpointProfile{
    pub fn supplied(input:MediaEndpointProfileInputs<'_>)->Result<Self,MediaError>{
        validate_routing(input.routing).map_err(|_|MediaError::Configuration)?;
        if input.properties.iter().any(|p|matches!(p.key,b"Client.ArcPort"|b"pwd"|b"token")){return Err(MediaError::Configuration);}
        for(timeout,max)in [(input.handshake_timeout,30),(input.partial_timeout,30),(input.io_timeout,30),(input.rpc_timeout,60)]{
            if timeout.is_zero()||timeout>Duration::from_secs(max){return Err(MediaError::Configuration);}
        }
        let encoded=encode_endpoint_connect(EndpointConnectInputs{content:b"bound-check",features:input.features,properties:input.properties}).map_err(|_|MediaError::Configuration)?;
        let routing=properties(input.routing)?;
        let total=encoded.expose_to_transport().len()+input.routing.iter().map(|p|p.key.len()+p.value.len()).sum::<usize>()
            +input.trust.authority().len()+input.trust.root_der_base64().len();
        if total>MAX_STREAM_BYTES{return Err(MediaError::Configuration);}
        Ok(Self{trust:input.trust,routing,features:input.features.iter().map(|p|Zeroizing::new(p.to_vec())).collect(),
            properties:properties(input.properties)?,key:input.reference_decode_key,
            handshake_timeout:input.handshake_timeout,partial_timeout:input.partial_timeout,io_timeout:input.io_timeout,rpc_timeout:input.rpc_timeout})
    }
}

/// Prepared from an original, positive, matched Accept operation, never from
/// caller-supplied decoded property data. Destination is bound before connect.
/// This is a new PHYSICAL media session: its own request sequence cannot collide
/// with the account ARC connection's existing shared pending allocator.
pub(super) struct MediaEndpointPlan{
    owner:AttemptOwner,ticket:CallControlTicket,scope:RpcConnectionScope,profile:Arc<MediaEndpointProfile>,
    reference:Zeroizing<String>,request:Option<RpcBytes>,id:i32,pending:bool,
}
impl fmt::Debug for MediaEndpointPlan{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("MediaEndpointPlan(<redacted>)")}}
impl Drop for MediaEndpointPlan{fn drop(&mut self){self.scope.close();}}
impl MediaEndpointPlan{
    /// Production creates the physical destination internally. It cannot reuse
    /// an account/another call's scope or collide with that scope's sequence 1.
    pub(super) fn prepare(reply:&OwnedCallOperationReply,ticket:&CallControlTicket,
        profile:Arc<MediaEndpointProfile>,original_arc_port:u16)->Result<Self,MediaError>{
        Self::capture(reply,ticket,RpcConnectionScope::fresh(),profile,original_arc_port)
    }
    /// Only source fixtures can inject scopes to test original-scope rejection.
    #[cfg(test)]
    pub(super) fn from_accept(reply:&OwnedCallOperationReply,ticket:&CallControlTicket,scope:RpcConnectionScope,
        profile:Arc<MediaEndpointProfile>,original_arc_port:u16)->Result<Self,MediaError>{
        Self::capture(reply,ticket,scope,profile,original_arc_port)
    }
    fn capture(reply:&OwnedCallOperationReply,ticket:&CallControlTicket,scope:RpcConnectionScope,
        profile:Arc<MediaEndpointProfile>,original_arc_port:u16)->Result<Self,MediaError>{
        let owner=ticket.owner();let account=ticket.connection_scope();
        if !ticket.is_live()||!scope.is_open()||scope.matches(&account)||!reply.original_call(ticket)
            ||reply.operation()!=CallOperation::Accept||!reply.accepted()||reply.native_id()!=ticket.native_id()||original_arc_port==0{return Err(MediaError::Scope);}
        let data=reply.properties_for(owner,&account).ok_or(MediaError::Scope)?;
        let get=|name:&[u8]|data.iter().find(|p|p.key==name).map(|p|p.value).ok_or(MediaError::MissingProperty);
        let endpoint=get(b"endpoint")?;let content=get(b"content")?;
        let decoded=decode_configuration(endpoint,profile.key.as_ref()).map_err(|_|MediaError::UnsupportedReference)?;
        let reference=parse_object_reference(decoded.expose_to_backend()).map_err(|_|MediaError::UnsupportedReference)?;
        reference.direct_raw_tcp().map_err(|_|MediaError::UnsupportedReference)?;
        if profile.routing.iter().find(|p|p.key.as_slice()==b"id").map(|p|p.value.as_slice())!=Some(reference.identity()){
            return Err(MediaError::UnsupportedReference);
        }
        let canonical=reference.encode_native().map_err(|_|MediaError::UnsupportedReference)?;
        let text=std::str::from_utf8(canonical.expose_to_backend()).map_err(|_|MediaError::UnsupportedReference)?;
        validate_explicit_profile(ConnectionInput{server_reference:text,authority:profile.trust.authority(),root_der_base64:profile.trust.root_der_base64(),
            enable_sni:profile.trust.enable_sni(),reference_decode_key:None}).map_err(|_|MediaError::UnsupportedReference)?;
        // Native Client.ArcPort is a logical Router listener port. It is DATA
        // here, not an OS UDP binding or a permission to dial an RTP endpoint.
        let port=Zeroizing::new(original_arc_port.to_string());let mut parameters=borrowed(&profile.properties);
        parameters.push(RpcProperty{key:b"Client.ArcPort",value:port.as_bytes()});
        let features:Vec<&[u8]>=profile.features.iter().map(|v|v.as_slice()).collect();
        let args=encode_endpoint_connect(EndpointConnectInputs{content,features:&features,properties:&parameters}).map_err(|_|MediaError::Configuration)?;
        let id=1;let request=encode_request(id,&borrowed(&profile.routing),CONNECT_ENDPOINT_METHOD.as_bytes(),args.expose_to_transport()).map_err(|_|MediaError::Configuration)?;
        // Consume one original-call admission only after all configuration and
        // request bounds pass. A prior/failed/dropped plan never reopens it.
        if !ticket.begin_media(){return Err(MediaError::AlreadyStarted);}
        Ok(Self{owner:owner.clone(),ticket:ticket.captured_copy(),scope,profile,reference:Zeroizing::new(text.to_owned()),request:Some(request),id,pending:true})
    }
    pub(super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(super) fn ticket(&self)->CallControlTicket{self.ticket.captured_copy()}
    pub(super) fn profile(&self)->&MediaEndpointProfile{&self.profile}
    pub(super) fn connection(&self)->Result<ConnectionInput<'_>,MediaError>{
        if !self.live(){return Err(MediaError::Scope);}
        Ok(ConnectionInput{server_reference:&self.reference,authority:self.profile.trust.authority(),root_der_base64:self.profile.trust.root_der_base64(),
            enable_sni:self.profile.trust.enable_sni(),reference_decode_key:None})
    }
    pub(super) fn request(&self)->Result<&[u8],MediaError>{
        if !self.live()||!self.pending{return Err(MediaError::Scope);}
        self.request.as_ref().map(RpcBytes::expose_to_transport).ok_or(MediaError::Scope)
    }
    pub(super) fn live(&self)->bool{self.ticket.is_live()&&self.scope.is_open()}
    pub(super) fn with_io_gate<T>(&self,work:impl FnOnce()->T)->Option<T>{self.ticket.with_media_gate(||self.scope.with_io_gate(work)).flatten()}
    pub(super) fn on_reply(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,bytes:&[u8])->Result<Option<OwnedMediaEndpointReply>,MediaError>{
        if owner!=&self.owner||!self.scope.matches(scope)||!self.live(){return Ok(None);}
        let envelope=decode_reply(bytes).map_err(|_|MediaError::Protocol)?;
        if !self.pending||envelope.call_id!=self.id{return Ok(None);}
        self.pending=false;self.request=None;
        let result=decode_endpoint_connect_result(envelope.result_code,envelope.result_stream).map_err(|e|if e==CodecError::RemoteFailure{MediaError::Rejected}else{MediaError::Protocol})?;
        Ok(Some(OwnedMediaEndpointReply{owner:self.owner.clone(),ticket:self.ticket.captured_copy(),scope:self.scope.clone(),profile:self.profile.clone(),
            features:result.features.iter().map(|v|Zeroizing::new(v.to_vec())).collect(),properties:properties(&result.properties)?}))
    }
}

/// Constructed only by the original pending Endpoint receive path. Raw audio
/// still needs an original native logical listener/route or supported Mpath
/// transport and SDP. Client.Host/ArcPort are not an IP/OS UDP socket grant.
pub(super) struct OwnedMediaEndpointReply{
    owner:AttemptOwner,ticket:CallControlTicket,scope:RpcConnectionScope,profile:Arc<MediaEndpointProfile>,
    features:Vec<Zeroizing<Vec<u8>>>,properties:Vec<Property>,
}
impl fmt::Debug for OwnedMediaEndpointReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedMediaEndpointReply(<redacted>)")}}
impl OwnedMediaEndpointReply{
    pub(super) fn matches(&self,ticket:&CallControlTicket,scope:&RpcConnectionScope,profile:&Arc<MediaEndpointProfile>)->bool{
        self.owner==*ticket.owner()&&self.ticket.same_call(ticket)&&self.scope.matches(scope)&&self.scope.is_open()&&ticket.is_live()&&Arc::ptr_eq(&self.profile,profile)
    }
    pub(super) fn properties(&self,ticket:&CallControlTicket,scope:&RpcConnectionScope,profile:&Arc<MediaEndpointProfile>)->Option<Vec<RpcProperty<'_>>>{
        self.matches(ticket,scope,profile).then(||borrowed(&self.properties))
    }
    pub(super) fn features(&self,ticket:&CallControlTicket,scope:&RpcConnectionScope,profile:&Arc<MediaEndpointProfile>)->Option<Vec<&[u8]>>{
        self.matches(ticket,scope,profile).then(||self.features.iter().map(|v|v.as_slice()).collect())
    }
}
#[cfg(test)]
#[path="media_endpoint_tests.rs"]
mod tests;
