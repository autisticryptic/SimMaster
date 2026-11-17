//! One purpose-bound UserEntry READ over the original qualified Agent path.
//! Fresh auxiliary IO is not SDK login/auth. Only a matched current reply may
//! construct OwnedUserEntryReply, never a cached UID, codec DATA or raw bool.
use super::{arc_session::UserEntryAuth,user_entry_action::{UserEntryAction,UserEntryObservation},user_entry_target::UserEntryTarget,
    user_entry_ownership::UserEntryLease,user_entry_data::{self,RecipientUri},router_connector::RouterPolicy,
    router_runtime::{RouterPath,RouterTimeouts},netstream_runtime::NetstreamPath,common_runtime::CommonPath,object_reference::ReferenceTarget,
    rpc_stream::RpcProperty,rpc_envelope::{encode_request,decode_reply},transport_packets::CallKind};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum UserEntryError{Scope,Configuration,Transport,Timeout,Protocol,Rejected,Ambiguous,Capacity,Realm}
// Preserve an ACTUAL typed original timeout through nested transport layers.
// A clock value, closed scope or generic IO error is not timeout evidence.
pub(super) fn router_error(error:super::router_runtime::RouterIoError)->UserEntryError{use super::{router_runtime::RouterIoError as E,runtime_connector::ConnectError,tls_engine::TlsError};match error{E::Timeout|E::Connect(ConnectError::Timeout)|E::Tls(TlsError::Timeout)=>UserEntryError::Timeout,_=>UserEntryError::Transport}}
pub(super) fn netstream_error(error:super::netstream_runtime::NetstreamError)->UserEntryError{use super::{netstream_runtime::NetstreamError as E,channel_session::SessionError,reliable_channel::ChannelError};match error{E::Timeout|E::Session(SessionError::Channel(ChannelError::Timeout))=>UserEntryError::Timeout,E::Router(error)=>router_error(error),_=>UserEntryError::Transport}}
pub(super) fn common_error(error:super::common_runtime::CommonIoError)->UserEntryError{use super::common_runtime::CommonIoError as E;match error{E::Timeout=>UserEntryError::Timeout,E::Netstream(error)=>netstream_error(error),_=>UserEntryError::Transport}}
struct Query{action:Arc<UserEntryAction>,id:i32}
impl fmt::Debug for Query{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryQuery(<redacted>)")}}
pub(super) struct OwnedUserEntryReply{query:Arc<Query>,accepted:bool,uid:Option<Zeroizing<Vec<u8>>>,observed_at:String}
impl fmt::Debug for OwnedUserEntryReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedUserEntryReply(<redacted>)")}}
impl OwnedUserEntryReply{
    pub(super) fn observe(&self,action:&Arc<UserEntryAction>)->Result<UserEntryObservation,UserEntryError>{
        if !Arc::ptr_eq(action,&self.query.action)||!action.live(){return Err(UserEntryError::Scope);}
        let kind=RecipientUri::parse(action.recipient()).map_err(|_|UserEntryError::Protocol)?.kind();
        Ok(UserEntryObservation{kind:match kind{user_entry_data::AidKind::Uid=>"uid",user_entry_data::AidKind::Phone=>"phone",user_entry_data::AidKind::Email=>"email",user_entry_data::AidKind::Username=>"username"},
            report:if !self.accepted{"rejected"}else if self.uid.is_some(){"reported"}else{"unreported"},observed_at:self.observed_at.clone()})
    }
    // Private result only, not a UserReadPermit and not an unknown-WRITE ACK.
    pub(super) fn resolved_uid_for(&self,action:&Arc<UserEntryAction>)->Option<&[u8]>{
        (Arc::ptr_eq(action,&self.query.action)&&action.live()&&self.accepted).then(||self.uid.as_deref().map(|v|v.as_slice())).flatten()
    }
}
pub(super) async fn resolve_original(action:Arc<UserEntryAction>,auth:Box<UserEntryAuth>,target:Arc<UserEntryTarget>,
    lease:&UserEntryLease,router:&RouterPolicy,access:&[RpcProperty<'_>],timeouts:RouterTimeouts,compress:Option<usize>)->Result<OwnedUserEntryReply,UserEntryError>{
    let owner=action.owner();let parent=action.parent();let child=lease.scope();let deadline=action.deadline().min(lease.deadline());
    let current=||action.live()&&auth.matches(owner,&parent)&&lease.matches(owner,&parent);
    if !current(){return Err(UserEntryError::Scope);}
    let uri=RecipientUri::parse(action.recipient()).map_err(|_|UserEntryError::Configuration)?;
    let realm=target.realm_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
    if uri.authority().is_some_and(|authority|authority!=realm){return Err(UserEntryError::Realm);}
    let query=Arc::new(Query{action:action.clone(),id:lease.id()});
    let operation=Box::pin(async{
        let reference=target.reference_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
        let ReferenceTarget::Direct(endpoints)=reference.target()else{return Err(UserEntryError::Configuration);};
        let endpoint=endpoints.first().ok_or(UserEntryError::Configuration)?;
        let arguments=user_entry_data::encode_aid_to_uid(&[uri.aid()]).map_err(|_|UserEntryError::Configuration)?;
        let token=auth.token_for(owner,&parent).ok_or(UserEntryError::Scope)?;
        let cc=target.cc_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
        // Original593F20 installs DeviceInfo.CC;59994C uses the exact Agent
        // name+112 as id. Logical endpoint data never selects OS/TLS authority.
        let metadata=[RpcProperty{key:b"id",value:reference.identity()},RpcProperty{key:b"token",value:token},RpcProperty{key:b"DeviceInfo.CC",value:cc}];
        let rpc=encode_request(query.id,&metadata,user_entry_data::AID_TO_UID_METHOD.as_bytes(),arguments.expose_to_transport()).map_err(|_|UserEntryError::Configuration)?;
        lease.arm(rpc.expose_to_transport().len()).map_err(|_|UserEntryError::Capacity)?;
        let route=RouterPath::open_user_entry(owner.clone(),child.clone(),&parent,router,access,timeouts,deadline).await.map_err(router_error)?;
        let stream=NetstreamPath::open(route,endpoint,0,deadline,timeouts.partial).await.map_err(netstream_error)?;
        let mut path=CommonPath::from_original(stream,timeouts.partial,compress).map_err(common_error)?;
        if !current(){return Err(UserEntryError::Scope);}
        if !path.send_call(owner,&child,CallKind::Request,rpc.expose_to_transport(),deadline).await.map_err(common_error)?{return Err(UserEntryError::Scope);}
        loop{
            if !current(){return Err(UserEntryError::Scope);}
            let Some(group)=path.receive_calls(owner,&child,deadline).await.map_err(common_error)?else{return Err(UserEntryError::Scope);};
            let calls=path.calls_for(owner,&child,&group).ok_or(UserEntryError::Scope)?;
            // Check the entire authenticated admitted group before publishing
            // any UID; a malformed later envelope suppresses the whole group.
            let mut matched=None;
            for call in calls{
                if call.kind!=CallKind::Reply{return Err(UserEntryError::Protocol);}
                let reply=decode_reply(call.payload.expose_to_transport()).map_err(|_|UserEntryError::Protocol)?;
                if reply.call_id==query.id{if matched.is_some(){return Err(UserEntryError::Protocol);}matched=Some(reply);}
            }
            let Some(reply)=matched else{continue;};
            let mut uid=None;let accepted=if reply.result_code!=0{false}else{
                let data=user_entry_data::decode_aid_to_uid(reply.result_code,reply.result_stream).map_err(|_|UserEntryError::Protocol)?;
                // A negative reply cannot deliver an otherwise valid UID.
                if data.accepted{for entry in data.entries{
                    if entry.aid.kind()!=uri.kind()||entry.aid.value()!=uri.value(){return Err(UserEntryError::Protocol);}
                    if uid.is_some(){return Err(UserEntryError::Ambiguous);}uid=Some(Zeroizing::new(entry.uid.to_vec()));
                }}data.accepted
            };
            if !current()||Instant::now()>=deadline{return Err(UserEntryError::Scope);}
            let result=OwnedUserEntryReply{query:query.clone(),accepted,uid,observed_at:chrono::Utc::now().to_rfc3339()};
            path.close();return Ok(result);
        }
    });
    let gate=action.gate();
    tokio::select!{biased;
        _=parent.session_ended()=>{lease.cancel();Err(UserEntryError::Scope)},
        _=gate.retired()=>{lease.cancel();Err(UserEntryError::Scope)},
        _=child.closed()=>Err(UserEntryError::Scope),
        result=tokio::time::timeout_at(deadline.into(),operation)=>result.map_err(|_|{lease.cancel();UserEntryError::Timeout})?,
    }
}
