//! One original account's bounded native IM + auxiliary RPC resources.
//! This is accounting, not authentication, a WRITE capability or a new scope.
//! Pending request bytes AND matched retained result bytes stay charged through
//! the last original holder's Drop. Closing IO never resets these resources.
use super::{rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,MAX_MAP_ENTRIES,MAX_STREAM_BYTES}};
use std::{fmt,sync::{Arc,Mutex}};
#[derive(Default)]
struct State{entries:usize,bytes:usize}
pub(super) struct NativeRpcBudget{state:Mutex<State>}
impl fmt::Debug for NativeRpcBudget{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeRpcBudget(<redacted>)")}}
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
impl NativeRpcBudget{
    // Constructed only with RpcConnectionScope::fresh; cloned scopes keep the
    // same budget, whereas a genuine new account incarnation has its own one.
    pub(super) fn original()->Arc<Self>{Arc::new(Self{state:Mutex::new(State::default())})}
    pub(super) fn reserve(self:&Arc<Self>,scope:&RpcConnectionScope,bytes:usize)->Result<Arc<NativeRpcReservation>,CodecError>{
        if !scope.is_open()||scope.session_revoked()||!scope.has_rpc_budget(self){return Err(CodecError::InvalidField);}
        if bytes>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        let mut state=lock(&self.state);let total=state.bytes.checked_add(bytes).ok_or(CodecError::TooLarge)?;
        if state.entries>=MAX_MAP_ENTRIES||total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        state.entries+=1;state.bytes=total;
        Ok(Arc::new(NativeRpcReservation{budget:self.clone(),state:Mutex::new(ReservationState{bytes,armed:bytes!=0,replied:false})}))
    }
    #[cfg(test)]pub(super) fn snapshot(&self)->(usize,usize){let s=lock(&self.state);(s.entries,s.bytes)}
}
struct ReservationState{bytes:usize,armed:bool,replied:bool}
pub(super) struct NativeRpcReservation{budget:Arc<NativeRpcBudget>,state:Mutex<ReservationState>}
impl fmt::Debug for NativeRpcReservation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeRpcReservation(<redacted>)")}}
impl NativeRpcReservation{
    pub(super) fn belongs_to(&self,scope:&RpcConnectionScope)->bool{scope.has_rpc_budget(&self.budget)}
    /// Auxiliary admission reserves its count before making a child scope/ID;
    /// only its original one-shot arm may add the now-encoded request bytes.
    pub(super) fn arm_request(&self,bytes:usize)->Result<(),CodecError>{
        let mut original=lock(&self.state);if original.armed||original.replied||bytes==0{return Err(CodecError::InvalidField);}
        let mut state=lock(&self.budget.state);let total=state.bytes.checked_add(bytes).ok_or(CodecError::TooLarge)?;
        if bytes>MAX_STREAM_BYTES||total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        state.bytes=total;original.bytes=bytes;original.armed=true;Ok(())
    }
    /// Only one matched private result may add retained bytes; caller booleans,
    /// decoded DATA and duplicate/late replies cannot replenish this budget.
    pub(super) fn retain_reply(&self,bytes:usize)->Result<(),CodecError>{
        let mut original=lock(&self.state);if !original.armed||original.replied{return Err(CodecError::InvalidField);}
        let own=original.bytes.checked_add(bytes).ok_or(CodecError::TooLarge)?;
        let mut state=lock(&self.budget.state);let total=state.bytes.checked_add(bytes).ok_or(CodecError::TooLarge)?;
        if own>MAX_STREAM_BYTES||total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        state.bytes=total;original.bytes=own;original.replied=true;Ok(())
    }
}
impl Drop for NativeRpcReservation{
    fn drop(&mut self){let bytes=lock(&self.state).bytes;let mut state=lock(&self.budget.state);
        state.entries=state.entries.saturating_sub(1);state.bytes=state.bytes.saturating_sub(bytes);
    }
}
