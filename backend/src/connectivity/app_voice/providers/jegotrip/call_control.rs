//! Original incoming-call control state, independent of SDK/hosting readiness.
//! Only the original authenticated receiver can register a call. Full native
//! call IDs and original identities survive all async command/receipt paths.
use super::{call_binding::OwnedCallNotice,rpc_calls::{CallNoticeKind,CallOperation,NativeCallId},
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcProperty,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES},session::AttemptOwner};
use std::{collections::{BTreeMap,VecDeque},fmt,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}};
use zeroize::Zeroizing;

const MAX_CALL_HISTORY:usize=128;
struct Property{key:Zeroizing<Vec<u8>>,value:Zeroizing<Vec<u8>>}
fn capture(input:&[RpcProperty<'_>])->Result<Vec<Property>,CodecError>{
    if input.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    let mut total=0usize;
    for(i,p)in input.iter().enumerate(){
        if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        total=total.checked_add(p.key.len()+p.value.len()).ok_or(CodecError::TooLarge)?;if total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        if input[..i].iter().any(|prior|prior.key==p.key){return Err(CodecError::DuplicateKey);}
    }
    Ok(input.iter().map(|p|Property{key:Zeroizing::new(p.key.to_vec()),value:Zeroizing::new(p.value.to_vec())}).collect())
}
fn borrowed(input:&[Property])->Vec<RpcProperty<'_>>{input.iter().map(|p|RpcProperty{key:&p.key,value:&p.value}).collect()}
fn valid_identifier(value:&[u8])->bool{
    !value.is_empty()&&value.len()<=MAX_FIELD_BYTES&&std::str::from_utf8(value).is_ok()&&!value.iter().any(u8::is_ascii_control)&&!value.iter().any(u8::is_ascii_whitespace)
        &&!value.iter().any(|b|matches!(b,b':'|b';'|b'@'|b'$'|b'*'))&&!value.windows(4).any(|w|w==b"ENC(")
}
pub(super) fn validate_routing(routing:&[RpcProperty<'_>])->Result<(),CodecError>{
    if routing.iter().any(|p|p.key==b"pwd"||p.key==b"token"){return Err(CodecError::InvalidField);}
    let get=|name:&[u8]|routing.iter().find(|p|p.key==name).map(|p|p.value);
    if !get(b"id").is_some_and(valid_identifier)||!get(b"domain").is_some_and(valid_identifier){return Err(CodecError::InvalidField);}
    capture(routing).map(|_|())
}
struct CallIdentity{live:AtomicBool,media_started:AtomicBool,remote_released:AtomicBool,gate:Mutex<()>,changed:tokio::sync::Notify}
pub struct CallControlTicket{owner:AttemptOwner,scope:RpcConnectionScope,native_id:NativeCallId,identity:Arc<CallIdentity>}
impl fmt::Debug for CallControlTicket{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CallControlTicket(<redacted>)")}}
impl CallControlTicket{
    pub fn owner(&self)->&AttemptOwner{&self.owner}
    pub fn connection_scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub fn native_id(&self)->NativeCallId{self.native_id}
    pub fn is_live(&self)->bool{self.scope.is_open()&&self.identity.live.load(Ordering::Acquire)}
    pub(super) fn remote_released(&self)->bool{self.identity.remote_released.load(Ordering::Acquire)}
    pub(super) fn same_call(&self,other:&Self)->bool{
        self.owner==other.owner&&self.scope.matches(&other.scope)&&self.native_id==other.native_id&&Arc::ptr_eq(&self.identity,&other.identity)
    }
    /// Lock order: call identity -> original account scope -> destination media
    /// scope (in the media caller). Nonblocking IO polls or bounded in-memory
    /// publication only; no blocking locks, close or await in the closure.
    /// Revocation serializes with each poll, not a copied ticket.
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.scope.matches(scope)&&scope.is_open()}
    pub(super) fn captured_copy(&self)->Self{Self{owner:self.owner.clone(),scope:self.scope.clone(),native_id:self.native_id,identity:self.identity.clone()}}
    pub(super) fn with_media_gate<T>(&self,work:impl FnOnce()->T)->Option<T>{
        let _gate=match self.identity.gate.lock(){Ok(gate)=>gate,Err(poison)=>{
            let _gate=poison.into_inner();self.identity.live.store(false,Ordering::Release);self.identity.changed.notify_waiters();return None;
        }};
        if !self.is_live(){return None;}
        self.scope.with_io_gate(||if self.identity.live.load(Ordering::Acquire){Some(work())}else{None}).flatten()
    }
    /// One original native call gets at most one media connection attempt.
    /// Failure/Drop cannot grant an automatic second use of its Accept result.
    pub(super) fn begin_media(&self)->bool{
        self.with_media_gate(||!self.identity.media_started.swap(true,Ordering::AcqRel)).unwrap_or(false)
    }
    pub(super) async fn revoked(&self){loop{
        let wake=self.identity.changed.notified();tokio::pin!(wake);wake.as_mut().enable();
        if !self.is_live(){return;}
        tokio::select!{biased;_=self.scope.closed()=>return,_=wake=>{}}
    }}
    pub(super) fn revoke(&self){
        let _gate=self.identity.gate.lock().unwrap_or_else(|poison|poison.into_inner());
        self.identity.live.store(false,Ordering::Release);self.identity.changed.notify_waiters();
    }
}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Phase{Received,Admitted,Checking,Checked,Accepting,Accepted,Releasing,Ended,Failed}
struct Slot{ticket:CallControlTicket,phase:Phase,override_id:Option<Zeroizing<Vec<u8>>>}
pub(super) struct CallBook{owner:AttemptOwner,scope:RpcConnectionScope,routing:Option<Vec<Property>>,calls:BTreeMap<u64,Slot>,retired:VecDeque<u64>}
impl CallBook{
    pub(super) fn new(owner:AttemptOwner,scope:RpcConnectionScope)->Self{Self{owner,scope,routing:None,calls:BTreeMap::new(),retired:VecDeque::new()}}
    pub(super) fn is_configured(&self)->bool{self.routing.is_some()}
    pub(super) fn acknowledgement_ready(&self,ticket:&CallControlTicket)->bool{
        self.slot(ticket).is_ok_and(|slot|slot.phase!=Phase::Received)
    }
    pub(super) fn configure(&mut self,routing:&[RpcProperty<'_>])->Result<(),CodecError>{
        if self.routing.is_some()||!self.scope.is_open(){return Err(CodecError::InvalidField);}
        validate_routing(routing)?;
        self.routing=Some(capture(routing)?);Ok(())
    }
    /// Duplicate invitations are not a new call/action capability. Remote
    /// release revokes the original identity before any queued command poll.
    pub(super) fn register(&mut self,notice:&OwnedCallNotice)->Result<Option<CallControlTicket>,CodecError>{
        if !notice.matches(&self.owner,&self.scope)||self.routing.is_none(){return Err(CodecError::InvalidField);}
        let id=notice.native_id().expose_to_backend();
        if self.retired.contains(&id){return Ok(None);}
        if let Some(slot)=self.calls.get_mut(&id){
            if notice.kind()==CallNoticeKind::Released{slot.ticket.identity.remote_released.store(true,Ordering::Release);slot.ticket.revoke();slot.phase=Phase::Ended;}
            if notice.kind()==CallNoticeKind::Invited{return Ok(None);}
            return Ok(Some(slot.ticket.captured_copy()));
        }
        if notice.kind()!=CallNoticeKind::Invited{return Ok(None);}
        // -1 is native's unallocated sentinel. Zero remains a full-width DATA
        // value, but is not admitted as a new control context by this policy.
        if id==0||id==u64::MAX{return Err(CodecError::InvalidCallId);}
        // Ended calls are replay history, not lifetime active-call capacity.
        // Keep a bounded tombstone window; never evict a live/releasing/failed
        // call whose cleanup is unconfirmed. Old tickets retain their revoked
        // Arc identity even after a completed slot is no longer in the table.
        if self.calls.len()>=MAX_CALL_HISTORY{
            if let Some(completed)=self.calls.iter().find(|(_,slot)|slot.phase==Phase::Ended).map(|(id,_)|*id){
                self.calls.remove(&completed);self.retired.push_back(completed);
                if self.retired.len()>MAX_CALL_HISTORY{self.retired.pop_front();}
            }
        }
        if self.calls.len()>=MAX_CALL_HISTORY{return Err(CodecError::InvalidCallId);}
        let data=notice.data_for(&self.owner,&self.scope).ok_or(CodecError::InvalidField)?;
        let override_id=data.properties.iter().find(|p|p.key==b"callServerEx.oid").map(|p|p.value);
        if override_id.is_some_and(|value|!valid_identifier(value)){return Err(CodecError::InvalidField);}
        let ticket=CallControlTicket{owner:self.owner.clone(),scope:self.scope.clone(),native_id:notice.native_id(),identity:Arc::new(CallIdentity{live:AtomicBool::new(true),media_started:AtomicBool::new(false),remote_released:AtomicBool::new(false),gate:Mutex::new(()),changed:tokio::sync::Notify::new()})};
        self.calls.insert(id,Slot{ticket:ticket.captured_copy(),phase:Phase::Received,override_id:override_id.map(|v|Zeroizing::new(v.to_vec()))});Ok(Some(ticket))
    }
    fn slot(&self,ticket:&CallControlTicket)->Result<&Slot,CodecError>{
        if !ticket.matches(&self.owner,&self.scope){return Err(CodecError::InvalidField);}
        self.calls.get(&ticket.native_id.expose_to_backend()).filter(|slot|Arc::ptr_eq(&slot.ticket.identity,&ticket.identity)).ok_or(CodecError::InvalidField)
    }
    pub(super) fn acknowledged(&mut self,ticket:&CallControlTicket,accepted:bool)->Result<(),CodecError>{
        let slot=self.slot(ticket)?;if slot.phase!=Phase::Received{return Ok(());}
        let slot=self.calls.get_mut(&ticket.native_id.expose_to_backend()).ok_or(CodecError::InvalidField)?;
        slot.phase=if accepted{Phase::Admitted}else{slot.ticket.revoke();Phase::Failed};Ok(())
    }
    pub(super) fn prepare<'a>(&'a self,ticket:&CallControlTicket,operation:CallOperation)->Result<Vec<RpcProperty<'a>>,CodecError>{
        let slot=self.slot(ticket)?;
        let allowed=match operation{
            CallOperation::Inform=>slot.phase==Phase::Admitted&&ticket.is_live(),
            CallOperation::Accept=>slot.phase==Phase::Checked&&ticket.is_live(),
            CallOperation::Release=>matches!(slot.phase,Phase::Received|Phase::Admitted|Phase::Checking|Phase::Checked|Phase::Accepting|Phase::Accepted|Phase::Failed),
        };
        if !allowed{return Err(CodecError::InvalidField);}
        let mut routing=borrowed(self.routing.as_ref().ok_or(CodecError::InvalidField)?);
        // Native duplicate/existence Inform precedes the returned per-call oid
        // handoff. Accept/release retain the actual original override when set.
        if operation!=CallOperation::Inform{if let Some(id)=&slot.override_id{routing.retain(|p|p.key!=b"id");routing.push(RpcProperty{key:b"id",value:id});}}
        Ok(routing)
    }
    pub(super) fn submitted(&mut self,ticket:&CallControlTicket,operation:CallOperation)->Result<(),CodecError>{
        self.prepare(ticket,operation)?;let slot=self.calls.get_mut(&ticket.native_id.expose_to_backend()).ok_or(CodecError::InvalidField)?;
        slot.phase=match operation{CallOperation::Inform=>Phase::Checking,CallOperation::Accept=>Phase::Accepting,CallOperation::Release=>{slot.ticket.revoke();Phase::Releasing}};Ok(())
    }
    pub(super) fn result(&mut self,ticket:&CallControlTicket,operation:CallOperation,accepted:bool)->bool{
        let Ok(slot)=self.slot(ticket)else{return false;};
        let expected=match operation{CallOperation::Inform=>Phase::Checking,CallOperation::Accept=>Phase::Accepting,CallOperation::Release=>Phase::Releasing};
        if slot.phase!=expected{return false;}
        if operation!=CallOperation::Release && !ticket.is_live(){
            if let Some(slot)=self.calls.get_mut(&ticket.native_id.expose_to_backend()){slot.ticket.revoke();slot.phase=Phase::Failed;}
            return false;
        }
        let Some(slot)=self.calls.get_mut(&ticket.native_id.expose_to_backend())else{return false;};
        if !accepted{slot.ticket.revoke();slot.phase=Phase::Failed;return true;}
        slot.phase=match operation{CallOperation::Inform=>Phase::Checked,CallOperation::Accept=>Phase::Accepted,CallOperation::Release=>Phase::Ended};true
    }
    pub(super) fn close(&mut self){for slot in self.calls.values(){slot.ticket.revoke();}self.calls.clear();self.retired.clear();self.routing=None;}
}
impl Drop for CallBook{fn drop(&mut self){self.close();}}

/// Constructed only by matched shared-ARC original pending entries. A true
/// acceptance is signaling DATA, never media-active or another-call readiness.
pub struct OwnedCallOperationReply{owner:AttemptOwner,scope:RpcConnectionScope,request_id:i32,ticket:CallControlTicket,operation:CallOperation,accepted:bool,valid_call:bool,properties:Vec<Property>}
impl fmt::Debug for OwnedCallOperationReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedCallOperationReply(<redacted>)")}}
impl OwnedCallOperationReply{
    pub(super) fn original(owner:AttemptOwner,scope:RpcConnectionScope,request_id:i32,ticket:CallControlTicket,operation:CallOperation,accepted:bool,valid_call:bool,properties:&[RpcProperty<'_>])->Result<Self,CodecError>{
        Ok(Self{owner,scope,request_id,ticket,operation,accepted,valid_call,properties:capture(properties)?})
    }
    pub(super) fn original_call(&self,ticket:&CallControlTicket)->bool{self.ticket.same_call(ticket)&&self.matches(ticket.owner(),&ticket.connection_scope())}
    pub fn request_id(&self)->i32{self.request_id}
    pub fn native_id(&self)->NativeCallId{self.ticket.native_id()}
    pub fn operation(&self)->CallOperation{self.operation}
    pub fn accepted(&self)->bool{self.accepted&&self.valid_call}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.scope.matches(scope)&&scope.is_open()&&self.ticket.matches(owner,scope)}
    pub fn properties_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<Vec<RpcProperty<'_>>>{if self.matches(owner,scope){Some(borrowed(&self.properties))}else{None}}
}

#[cfg(test)]
#[path="call_control_tests.rs"]
mod tests;
