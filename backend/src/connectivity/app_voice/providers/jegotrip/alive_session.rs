//! Original main-session/agent/pending binding for AccountAlive, not a public grant.
use super::{rpc_pending::RpcConnectionScope,session::AttemptOwner,rpc_stream::{CodecError,Reader,read_footer,RpcProperty,MAX_FIELD_BYTES},rpc_envelope::{encode_request,decode_reply},rpc_alive::{self,RegisterAliveArguments}};
use std::{fmt,sync::{Arc,Mutex},time::{Duration,Instant}};
use zeroize::Zeroizing;
fn lock<T>(value:&Mutex<T>)->std::sync::MutexGuard<'_,T>{value.lock().unwrap_or_else(|p|p.into_inner())}
struct Identity{uid:Zeroizing<Vec<u8>>,selector:Zeroizing<Vec<u8>>,token:Zeroizing<Vec<u8>>,main:Option<Zeroizing<Vec<u8>>>}
struct State{identity:Option<Identity>,claimed:bool,stopped:bool,phase:&'static str,interval:u32,agent:Option<RpcConnectionScope>,activity:Option<Instant>}
pub(super) struct AliveLink{owner:AttemptOwner,main:RpcConnectionScope,state:Mutex<State>}
impl fmt::Debug for AliveLink{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AliveLink(<redacted>)")}}
impl AliveLink{
    pub(super) fn original(owner:AttemptOwner,main:RpcConnectionScope)->Arc<Self>{Arc::new(Self{owner,main,state:Mutex::new(State{identity:None,claimed:false,stopped:false,phase:"unsupported",interval:0,agent:None,activity:None})})}
    pub(super) fn bind_authenticated(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,uid:&[u8],selector:&[u8],token:&[u8],main:Option<&[u8]>)->Result<(),CodecError>{
        if !self.matches(owner,scope)||[uid,selector,token].iter().any(|v|v.is_empty()||v.len()>MAX_FIELD_BYTES)||main.is_some_and(|v|v.len()>MAX_FIELD_BYTES){return Err(CodecError::InvalidField);}
        let mut state=lock(&self.state);if state.identity.is_some()||state.stopped{return Err(CodecError::InvalidField);}
        state.identity=Some(Identity{uid:Zeroizing::new(uid.to_vec()),selector:Zeroizing::new(selector.to_vec()),token:Zeroizing::new(token.to_vec()),main:main.map(|v|Zeroizing::new(v.to_vec()))});Ok(())
    }
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.main.matches(scope)&&self.main.is_open()&&!self.main.session_revoked()}
    pub(super) fn current_token(&self,token:&[u8])->bool{lock(&self.state).identity.as_ref().is_some_and(|id|id.token.as_slice()==token)}
    pub(super) fn main_scope(&self)->RpcConnectionScope{self.main.clone()}
    pub(super) fn live(&self)->bool{self.main.is_open()&&!self.main.session_revoked()}
    pub(super) fn claim(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,interval:u32)->Result<(),&'static str>{
        if !self.matches(owner,scope){return Err("app_account_alive_scope_invalid");}
        let mut state=lock(&self.state);if state.claimed||state.identity.is_none(){return Err("app_account_alive_scope_invalid");}
        state.claimed=true;state.interval=interval;state.phase="starting";Ok(())
    }
    pub(super) fn claimed(&self)->bool{lock(&self.state).claimed}
    pub(super) fn label(&self)->&'static str{
        if !self.live(){return "stopped";}
        let state=lock(&self.state);if state.phase=="registered"&&!state.agent.as_ref().is_some_and(RpcConnectionScope::is_open){"retrying"}else{state.phase}
    }
    pub(super) fn idle(&self,phase:&'static str){let mut state=lock(&self.state);state.phase=phase;state.activity=None;if let Some(agent)=state.agent.take(){agent.close();}}
    pub(super) fn close(&self){let mut state=lock(&self.state);state.stopped=true;state.phase="stopped";state.activity=None;state.identity=None;if let Some(scope)=state.agent.take(){scope.close();}}
    pub(super) fn agent(self:&Arc<Self>)->Result<AliveAgent,&'static str>{
        if !self.live(){return Err("app_account_alive_scope_invalid");}
        let mut state=lock(&self.state);if !state.claimed||state.stopped||state.identity.is_none()||state.agent.as_ref().is_some_and(RpcConnectionScope::is_open){return Err("app_account_alive_scope_invalid");}
        let scope=RpcConnectionScope::fresh();state.agent=Some(scope.clone());state.activity=None;state.phase="registering";
        Ok(AliveAgent{link:self.clone(),scope,next_id:Some(1),pending:None,callbacks:std::collections::BTreeMap::new(),retired_callback:0})
    }
    pub(super) fn check(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,token:&[u8],now:Instant)->(bool,u32){
        if !self.matches(owner,scope){return (false,0);}
        let mut state=lock(&self.state);
        if state.phase!="registered"||state.interval==0||!state.identity.as_ref().is_some_and(|id|id.token.as_slice()==token)
            ||!state.agent.as_ref().is_some_and(RpcConnectionScope::is_open){return (false,0);}
        if !state.activity.is_some_and(|at|now.checked_duration_since(at).is_some_and(|elapsed|elapsed<Duration::from_secs(u64::from(state.interval)*2))){return (false,0);}
        // 5A3E74 returns the configured seconds and records this original
        // checkAlive activity; it does not create an SDK/hosting/media grant.
        state.activity=Some(now);(true,state.interval)
    }
    pub(super) fn due(&self,now:Instant)->Result<Instant,&'static str>{
        if !self.live(){return Err("app_account_alive_scope_invalid");}
        let state=lock(&self.state);
        Ok(state.activity.and_then(|at|at.checked_add(Duration::from_secs(u64::from(state.interval)*2))).unwrap_or(now))
}
struct Pending{id:i32,deadline:Instant}
struct AgentGuard{scope:RpcConnectionScope,success:bool}
impl Drop for AgentGuard{fn drop(&mut self){if !self.success{self.scope.close();}}}
pub(super) enum AliveResult{Accepted,Rejected,SessionInvalid}
pub(super) struct AliveAgent{link:Arc<AliveLink>,scope:RpcConnectionScope,next_id:Option<i32>,pending:Option<Pending>,
    callbacks:std::collections::BTreeMap<i32,([u8;32],Vec<u8>)>,retired_callback:i32}
impl fmt::Debug for AliveAgent{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AliveAgent(<redacted>)")}}
impl AliveAgent{
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    fn original(&self)->bool{self.link.live()&&self.scope.is_open()&&lock(&self.link.state).agent.as_ref().is_some_and(|s|s.matches(&self.scope))}
    pub(super) fn prepare(&mut self,wall_ms:u64,now:Instant,deadline:Instant)->Result<super::rpc_stream::RpcBytes,&'static str>{
        if !self.original()||self.pending.is_some()||now>=deadline{return Err("app_account_alive_scope_invalid");}
        let id=self.next_id.ok_or("app_account_alive_request_limit")?;let mut state=lock(&self.link.state);
        let identity=state.identity.as_ref().ok_or("app_account_alive_scope_invalid")?;
        let arguments=rpc_alive::encode_register_alive(&RegisterAliveArguments{user_id:&identity.uid,selected_session_id:&identity.selector,request_timestamp_ms:wall_ms,interval_seconds:state.interval}).map_err(|_|"app_account_alive_protocol_invalid")?;
        let mut metadata=vec![RpcProperty{key:b"id",value:b"AccountAlive"},RpcProperty{key:b"token",value:&identity.token}];
        if let Some(main)=&identity.main{metadata.push(RpcProperty{key:b"mainAccount",value:main});}
        metadata.sort_by(|a,b|a.key.cmp(b.key));
        let request=encode_request(id,&metadata,rpc_alive::REGISTER_ALIVE_METHOD.as_bytes(),arguments.expose_to_transport()).map_err(|_|"app_account_alive_protocol_invalid")?;
        state.phase="registering";state.activity=None;self.next_id=id.checked_add(1);self.pending=Some(Pending{id,deadline});Ok(request)
    }
    pub(super) fn response(&mut self,bytes:&[u8],now:Instant)->Result<Option<AliveResult>,&'static str>{
        let mut guard=AgentGuard{scope:self.scope.clone(),success:false};
        if !self.original(){return Err("app_account_alive_scope_invalid");}
        let reply=decode_reply(bytes).map_err(|_|"app_account_alive_protocol_invalid")?;
        let Some(pending)=self.pending.as_ref()else{guard.success=true;return Ok(None);};if reply.call_id!=pending.id{guard.success=true;return Ok(None);}
        if now>=pending.deadline{return Err("app_account_alive_timeout");}
        // Read only bounded native footer fields, never expose their text.
        if reply.result_code!=0{return Err("app_account_alive_protocol_invalid");}
        let mut reader=Reader::new(reply.result_stream).map_err(|_|"app_account_alive_protocol_invalid")?;
        let accepted=reader.boolean().map_err(|_|"app_account_alive_protocol_invalid")?;
        let footer=read_footer(&mut reader).map_err(|_|"app_account_alive_protocol_invalid")?;reader.finish().map_err(|_|"app_account_alive_protocol_invalid")?;
        let invalid=footer.reason.starts_with(b"token-error")||footer.reason.starts_with(b"session-error");
        let accepted=accepted&&footer.reason.is_empty();self.pending=None;
        if invalid{
            self.link.idle("session_invalid");self.link.main.mark_session_revoked();guard.success=true;return Ok(Some(AliveResult::SessionInvalid));
        }
        let mut state=lock(&self.link.state);
        if !self.scope.is_open()||!self.link.live()||!state.agent.as_ref().is_some_and(|s|s.matches(&self.scope)){return Err("app_account_alive_scope_invalid");}
        state.phase=if accepted{"registered"}else{"retrying"};state.activity=accepted.then_some(now);
        guard.success=true;Ok(Some(if accepted{AliveResult::Accepted}else{AliveResult::Rejected}))
    }
    pub(super) fn pending_deadline(&self)->Option<Instant>{self.pending.as_ref().map(|p|p.deadline)}
    pub(super) fn validate_incoming(&self,kind:super::transport_packets::CallKind,bytes:&[u8],policy:&super::native_receiver::NativeReceiverPolicy)->Result<(),&'static str>{
        let mut guard=AgentGuard{scope:self.scope.clone(),success:false};
        let error="app_account_alive_protocol_invalid";
        match kind{
            super::transport_packets::CallKind::Reply=>{
                let reply=decode_reply(bytes).map_err(|_|error)?;if reply.result_code!=0{return Err(error);}
                let mut r=Reader::new(reply.result_stream).map_err(|_|error)?;r.boolean().map_err(|_|error)?;read_footer(&mut r).map_err(|_|error)?;r.finish().map_err(|_|error)?;
            }
            super::transport_packets::CallKind::Request=>{
                let request=super::rpc_envelope::decode_request(bytes).map_err(|_|error)?;
                if request.call_id<=0||request.method!=super::rpc_client_ex::CHECK_ALIVE_METHOD.as_bytes(){return Err(error);}
                policy.validate(super::native_receiver::ReceiverObject::Message,&request.metadata,super::native_receiver::ReceiverTransport::Direct).map_err(|_|error)?;
                Reader::versioned(request.arguments).and_then(|reader|reader.finish()).map_err(|_|error)?;
            }
            _=>return Err(error),
        }guard.success=true;Ok(())
    }
    pub(super) fn callback(&mut self,bytes:&[u8],now:Instant,policy:&super::native_receiver::NativeReceiverPolicy)->Result<super::rpc_stream::RpcBytes,&'static str>{
        let mut guard=AgentGuard{scope:self.scope.clone(),success:false};
        if !self.original(){return Err("app_account_alive_scope_invalid");}
        self.validate_incoming(super::transport_packets::CallKind::Request,bytes,policy)?;
        let request=super::rpc_envelope::decode_request(bytes).map_err(|_|"app_account_alive_protocol_invalid")?;
        let hash:[u8;32]=ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().try_into().map_err(|_|"app_account_alive_protocol_invalid")?;
        if let Some((old,result))=self.callbacks.get(&request.call_id){
            if *old!=hash{return Err("app_account_alive_protocol_invalid");}
            let reply=super::rpc_envelope::encode_reply(request.call_id,0,result).map_err(|_|"app_account_alive_protocol_invalid")?;
            guard.success=true;return Ok(reply);
        }
        if request.call_id<=self.retired_callback{return Err("app_account_alive_protocol_invalid");}
        let token=lock(&self.link.state).identity.as_ref().ok_or("app_account_alive_scope_invalid")?.token.clone();
        let (accepted,interval)=self.link.check(&self.link.owner,&self.link.main,&token,now);
        let mut writer=super::rpc_stream::Writer::new();writer.boolean(accepted).map_err(|_|"app_account_alive_protocol_invalid")?;
        writer.u32(interval).map_err(|_|"app_account_alive_protocol_invalid")?;writer.bytes(b"").map_err(|_|"app_account_alive_protocol_invalid")?;
        let result=writer.finish();let reply=super::rpc_envelope::encode_reply(request.call_id,0,result.expose_to_transport()).map_err(|_|"app_account_alive_protocol_invalid")?;
        if self.callbacks.len()==128{if let Some((id,_))=self.callbacks.pop_first(){self.retired_callback=self.retired_callback.max(id);}}
        self.callbacks.insert(request.call_id,(hash,result.expose_to_transport().to_vec()));guard.success=true;Ok(reply)
    }
}
impl Drop for AliveAgent{fn drop(&mut self){self.scope.close();let mut state=lock(&self.link.state);if state.agent.as_ref().is_some_and(|s|s.matches(&self.scope)){state.agent=None;state.activity=None;if state.phase!="stopped"&&state.phase!="session_invalid"{state.phase="retrying";}}}}
