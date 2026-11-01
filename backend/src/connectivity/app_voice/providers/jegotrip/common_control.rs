//! Native Common controls (DDA530): Ping2 -> empty Pong14, Release4 retires
//! the original connection, and Pong samples only an actually pending local
//! ping. This private state machine neither authenticates nor owns physical IO.
use super::{common_packets::{CommonError,CommonMessage},transport_packets::CallKind};
use std::{collections::VecDeque,fmt,time::{Duration,Instant}};

const MAX_PINGS:usize=16;
const MAX_PONGS:usize=64;
const MAX_CALLS:usize=256;
const MAX_BYTES:usize=1024*1024;
const MAX_PING_WAIT:Duration=Duration::from_secs(30);
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum ControlError{Common(CommonError),Configuration,Bound,Timeout,ClockRollback,RemoteReleased,Closed}
struct PendingPing{sent:Instant,deadline:Instant}
pub(super) struct ControlPlan{pub calls:Vec<CommonMessage>,pub pong_deadlines:Vec<Instant>}
impl fmt::Debug for ControlPlan{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ControlPlan(<redacted>)")}}
pub(super) struct CommonControls{pending:VecDeque<PendingPing>,delay_ms:Option<u64>,last_now:Instant,closed:bool}
impl fmt::Debug for CommonControls{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommonControls(<redacted>)")}}
impl CommonControls{
    pub fn new(now:Instant)->Self{Self{pending:VecDeque::new(),delay_ms:None,last_now:now,closed:false}}
    pub fn close(&mut self){self.closed=true;self.pending.clear();self.delay_ms=None;}
    pub fn delay_ms(&self)->Option<u64>{if self.closed{None}else{self.delay_ms}}
    pub fn deadline(&self)->Option<Instant>{self.pending.iter().map(|ping|ping.deadline).min()}
    pub fn tick(&mut self,now:Instant)->Result<(),ControlError>{
        let result=if self.closed{Err(ControlError::Closed)}else if now<self.last_now{Err(ControlError::ClockRollback)}
            else if self.deadline().is_some_and(|end|now>=end){Err(ControlError::Timeout)}else{self.last_now=now;Ok(())};
        if result.is_err(){self.close();}result
    }
    /// Admit before sending on the original NetStream; cancelled/failed IO must
    ///close this state, never migrate or retransmit a new Common Ping request.
    pub fn prepare_ping(&mut self,deadline:Instant,now:Instant)->Result<(),ControlError>{
        self.tick(now)?;
        if deadline.checked_duration_since(now).is_none_or(|left|left.is_zero()||left>MAX_PING_WAIT){return Err(ControlError::Configuration);}
        if self.pending.len()>=MAX_PINGS{return Err(ControlError::Bound);}
        self.pending.push_back(PendingPing{sent:now,deadline});Ok(())
    }
    pub fn on_messages(&mut self,messages:Vec<CommonMessage>,now:Instant)->Result<ControlPlan,ControlError>{
        let result=self.consume(messages,now);if result.is_err(){self.close();}result
    }
    fn consume(&mut self,messages:Vec<CommonMessage>,now:Instant)->Result<ControlPlan,ControlError>{
        self.tick(now)?;
        if messages.len()>MAX_CALLS{return Err(ControlError::Bound);}
        let mut plan=ControlPlan{calls:Vec::new(),pong_deadlines:Vec::new()};let mut bytes=0usize;
        for message in messages{
            if now>=message.deadline{return Err(ControlError::Timeout);}
            if !matches!(message.kind,CallKind::Request|CallKind::Reply)&&!message.payload.expose_to_transport().is_empty(){return Err(ControlError::Common(CommonError::Packet));}
            match message.kind{
                CallKind::Ping=>{
                    if plan.pong_deadlines.len()>=MAX_PONGS{return Err(ControlError::Bound);}
                    plan.pong_deadlines.push(message.deadline);
                }
                CallKind::Pong=>if let Some(ping)=self.pending.pop_front(){
                    let sample=u64::try_from(now.checked_duration_since(ping.sent).ok_or(ControlError::ClockRollback)?.as_millis()).map_err(|_|ControlError::Bound)?;
                    // DDA850: ceil(old/2)+floor(sample/2), not Router timing.
                    self.delay_ms=Some(self.delay_ms.map_or(sample,|old|old-(old>>1)+(sample>>1)));
                },
                CallKind::Release=>return Err(ControlError::RemoteReleased),
                CallKind::Request|CallKind::Reply=>{
                    if message.payload.expose_to_transport().is_empty(){return Err(ControlError::Common(CommonError::Packet));}
                    bytes=bytes.checked_add(message.payload.expose_to_transport().len()).ok_or(ControlError::Bound)?;
                    if bytes>MAX_BYTES{return Err(ControlError::Bound);}plan.calls.push(message);
                }
            }
        }
        Ok(plan)
    }
}
impl Drop for CommonControls{fn drop(&mut self){self.close();}}

#[cfg(test)]
#[path="common_control_tests.rs"]
mod tests;
