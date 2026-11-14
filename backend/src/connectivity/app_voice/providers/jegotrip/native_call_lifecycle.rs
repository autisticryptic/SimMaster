//! Native built-in audio CallEx parameters and original-call timer DATA.
//! The installed timer is explicit (seconds -> native signed milliseconds), not
//! an AccountAlive interval. Neither timestamps nor parameter maps grant media.
use super::{call_control::{CallControlTicket,OwnedCallOperationReply},rpc_calls::CallOperation,rpc_stream::RpcProperty};
use serde::{Deserialize,Serialize};
use std::{fmt,time::{Duration,Instant}};
const INVALID:&str="app_account_call_configuration_invalid";
const SCOPE:&str="app_account_call_scope_invalid";
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCallLifecycleConfig { pub session_timer_seconds:u32 }
impl fmt::Debug for NativeCallLifecycleConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeCallLifecycleConfig(<redacted>)")}}
#[derive(Clone,Copy)]
pub(super) struct NativeCallPolicy{milliseconds:u32}
impl NativeCallPolicy{
    pub(super) fn capture(config:NativeCallLifecycleConfig)->Result<Self,&'static str>{
        let milliseconds=config.session_timer_seconds.checked_mul(1000).filter(|value|*value<=i32::MAX as u32).ok_or(INVALID)?;
        Ok(Self{milliseconds})
    }
    pub(super) fn bind(self,ticket:&CallControlTicket)->NativeCallTiming{
        NativeCallTiming{ticket:ticket.captured_copy(),milliseconds:self.milliseconds.to_string(),period:Duration::from_millis(self.milliseconds.into()),
            started:None,accepted:None,connected:None,next:None,accept_reported:false,connection_reported:false}
    }
}
pub(super) struct NativeCallTiming{
    ticket:CallControlTicket,milliseconds:String,period:Duration,started:Option<Instant>,accepted:Option<Instant>,connected:Option<Instant>,next:Option<Instant>,
    accept_reported:bool,connection_reported:bool,
}
impl fmt::Debug for NativeCallTiming{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeCallTiming(<redacted>)")}}
impl NativeCallTiming{
    fn original(&self,ticket:&CallControlTicket)->bool{self.ticket.same_call(ticket)&&ticket.is_live()}
    pub(super) fn accept_properties<'a>(&'a self,ticket:&CallControlTicket,sdp:&'a[u8])->Result<Vec<RpcProperty<'a>>,&'static str>{
        if !self.original(ticket)||self.started.is_some()||sdp.is_empty(){return Err(SCOPE);}
        // This is explicitly the built-in single audio engine, with no external
        // CallI stream attachments. Legacy/external media never uses this path.
        Ok(vec![RpcProperty{key:b"RseEx.SDP",value:sdp},RpcProperty{key:b"mediaType",value:b"audio"},
            RpcProperty{key:b"keepAlive",value:self.milliseconds.as_bytes()},RpcProperty{key:b"SDK.IsNew",value:b"1"}])
    }
    pub(super) fn begin_accept(&mut self,ticket:&CallControlTicket,now:Instant)->Result<(),&'static str>{
        if !self.original(ticket)||self.started.is_some(){return Err(SCOPE);}self.started=Some(now);Ok(())
    }
    pub(super) fn accepted(&mut self,ticket:&CallControlTicket,reply:&OwnedCallOperationReply,now:Instant)->Result<(),&'static str>{
        if !self.original(ticket)||self.accepted.is_some()||!reply.original_call(ticket)||reply.operation()!=CallOperation::Accept||!reply.accepted(){return Err(SCOPE);}
        let start=self.started.ok_or(SCOPE)?;now.checked_duration_since(start).ok_or(SCOPE)?;
        self.next=if self.period.is_zero(){None}else{Some(now.checked_add(self.period).ok_or(SCOPE)?)};
        self.accepted=Some(now);Ok(())
    }
    pub(super) fn connected(&mut self,ticket:&CallControlTicket,now:Instant)->Result<(),&'static str>{
        if !self.original(ticket)||self.connected.is_some(){return Err(SCOPE);}
        now.checked_duration_since(self.accepted.ok_or(SCOPE)?).ok_or(SCOPE)?;self.connected=Some(now);Ok(())
    }
    pub(super) fn next(&self,ticket:&CallControlTicket,now:Instant)->Result<Option<Instant>,&'static str>{
        if !self.original(ticket){return Err(SCOPE);}
        if self.connected.is_some()&&!self.connection_reported{return Ok(Some(now));}
        Ok(self.next)
    }
    /// Called once by the owning worker immediately before submitting. One-shot
    /// duration fields are consumed at that boundary, never replayed on failure.
    pub(super) fn take_due(&mut self,ticket:&CallControlTicket,now:Instant)->Result<Option<Vec<(Vec<u8>,Vec<u8>)>>,&'static str>{
        if !self.original(ticket){return Err(SCOPE);}
        let accepted=self.accepted.ok_or(SCOPE)?;now.checked_duration_since(accepted).ok_or(SCOPE)?;
        let periodic=self.next.is_some_and(|at|now>=at);let connected=self.connected.is_some()&&!self.connection_reported;
        if !periodic&&!connected{return Ok(None);}
        let mut fields=vec![(b"keepAlive".to_vec(),self.milliseconds.as_bytes().to_vec())];
        if !self.accept_reported{
            let duration=accepted.checked_duration_since(self.started.ok_or(SCOPE)?).ok_or(SCOPE)?.as_millis();
            fields.push((b"RseEx.AcceptDuration".to_vec(),duration.to_string().into_bytes()));self.accept_reported=true;
        }
        if connected{
            let duration=self.connected.unwrap().checked_duration_since(self.started.ok_or(SCOPE)?).ok_or(SCOPE)?.as_millis();
            fields.push((b"RseEx.ConnDuration".to_vec(),duration.to_string().into_bytes()));self.connection_reported=true;
        }
        if periodic{
            // Skip missed periods rather than sending a catch-up burst. An
            // immediate connected report does not reset the original timer.
            let previous=self.next.ok_or(SCOPE)?;let elapsed=now.duration_since(previous).as_nanos();
            let steps=elapsed/self.period.as_nanos()+1;
            let step=u32::try_from(steps).map_err(|_|SCOPE)?;
            self.next=Some(previous.checked_add(self.period.checked_mul(step).ok_or(SCOPE)?).ok_or(SCOPE)?);
        }
        Ok(Some(fields))
    }
}
