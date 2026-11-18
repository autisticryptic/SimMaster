//! Owned auxiliary operation slots of one original account. Closing a slot
//! cannot release its owner: shutdown waits for the guard AFTER physical Drop.
use super::{rpc_pending::RpcConnectionScope,session::AttemptOwner,rpc_stream::CodecError};
use std::{collections::BTreeMap,fmt,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::Instant};
const MAX_AUXILIARY_OPERATIONS:usize=4;
const MAX_PENDING_BYTES:usize=128*1024;
struct Slot{scope:RpcConnectionScope,bytes:usize,budget:Option<Arc<super::native_rpc_budget::NativeRpcReservation>>}
struct State{retired:bool,next:Option<i32>,scopes:BTreeMap<i32,Slot>,bytes:usize}
pub(super) struct UserEntryBook{owner:AttemptOwner,parent:RpcConnectionScope,state:Mutex<State>,changed:tokio::sync::Notify}
impl fmt::Debug for UserEntryBook{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryBook(<redacted>)")}}
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
impl UserEntryBook{
    pub(super) fn original(owner:AttemptOwner,parent:RpcConnectionScope)->Arc<Self>{Arc::new(Self{owner,parent,state:Mutex::new(State{retired:false,next:Some(1),scopes:BTreeMap::new(),bytes:0}),changed:tokio::sync::Notify::new()})}
    pub(super) fn acquire(self:&Arc<Self>,owner:&AttemptOwner,parent:&RpcConnectionScope,deadline:Instant)->Result<UserEntryLease,CodecError>{
        if &self.owner!=owner||!self.parent.matches(parent)||!parent.is_open()||parent.session_revoked()||Instant::now()>=deadline{return Err(CodecError::InvalidField);}
        let mut s=lock(&self.state);if s.retired||s.scopes.len()>=MAX_AUXILIARY_OPERATIONS{return Err(CodecError::TooLarge);}let id=s.next.ok_or(CodecError::TooLarge)?;
        let budget=self.parent.reserve_native_rpc(0)?;s.next=id.checked_add(1);
        let scope=RpcConnectionScope::fresh();s.scopes.insert(id,Slot{scope:scope.clone(),bytes:0,budget:Some(budget)});Ok(UserEntryLease{book:self.clone(),id,scope,deadline,armed:AtomicBool::new(false)})
    }
    pub(super) fn retire(&self){let mut s=lock(&self.state);s.retired=true;for slot in s.scopes.values(){slot.scope.close();}self.changed.notify_waiters();}
    pub(super) async fn retired_and_empty(&self){self.retire();loop{let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();if lock(&self.state).scopes.is_empty(){return;}changed.await;}}
    pub(super) fn live(&self)->bool{self.parent.is_open()&&!self.parent.session_revoked()&&!lock(&self.state).retired}
    #[cfg(test)]pub(super) fn active(&self)->usize{lock(&self.state).scopes.len()}
    #[cfg(test)]pub(super) fn retained_bytes(&self)->usize{lock(&self.state).bytes}
}
pub(super) struct UserEntryLease{book:Arc<UserEntryBook>,id:i32,scope:RpcConnectionScope,deadline:Instant,armed:AtomicBool}
impl fmt::Debug for UserEntryLease{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryLease(<redacted>)")}}
impl UserEntryLease{
    pub(super) fn id(&self)->i32{self.id}
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->bool{
        &self.book.owner==owner&&self.book.parent.matches(parent)&&self.book.live()&&self.scope.is_open()&&Instant::now()<self.deadline
            &&lock(&self.book.state).scopes.get(&self.id).is_some_and(|s|s.scope.matches(&self.scope))
    }
    pub(super) fn arm(&self,bytes:usize)->Result<(),CodecError>{
        if bytes==0||bytes>MAX_PENDING_BYTES||self.armed.load(Ordering::Acquire){return Err(CodecError::TooLarge);}
        let mut s=lock(&self.book.state);if s.retired||!self.scope.is_open()||!self.book.parent.is_open()||self.book.parent.session_revoked()||Instant::now()>=self.deadline{return Err(CodecError::InvalidField);}
        let total=s.bytes.checked_add(bytes).ok_or(CodecError::TooLarge)?;if total>MAX_PENDING_BYTES{return Err(CodecError::TooLarge);}
        let slot=s.scopes.get_mut(&self.id).ok_or(CodecError::InvalidField)?;
        if !slot.scope.matches(&self.scope)||self.armed.load(Ordering::Acquire){return Err(CodecError::InvalidField);}
        slot.budget.as_ref().ok_or(CodecError::InvalidField)?.arm_request(bytes)?;
        self.armed.store(true,Ordering::Release);slot.bytes=bytes;s.bytes=total;Ok(())
    }
    pub(super) fn retain_reply(&self,bytes:usize)->Result<Arc<super::native_rpc_budget::NativeRpcReservation>,CodecError>{
        let s=lock(&self.book.state);let slot=s.scopes.get(&self.id).ok_or(CodecError::InvalidField)?;
        if s.retired||!slot.scope.matches(&self.scope)||!self.scope.is_open()||!self.book.parent.is_open()||self.book.parent.session_revoked()||Instant::now()>=self.deadline{return Err(CodecError::InvalidField);}
        let budget=slot.budget.as_ref().ok_or(CodecError::InvalidField)?.clone();budget.retain_reply(bytes)?;Ok(budget)
    }
    pub(super) fn cancel(&self){self.scope.close();}
}
impl Drop for UserEntryLease{fn drop(&mut self){self.scope.close();let mut s=lock(&self.book.state);if let Some(slot)=s.scopes.remove(&self.id){s.bytes=s.bytes.saturating_sub(slot.bytes);}drop(s);self.book.changed.notify_waiters();}}
