//! Independent owned AccountAlive task. It never logs in or reacquires a main
//! SDK session. Explicit legacy transport approval is separate from SDK TLS.
use super::{alive_profile::AlivePolicy,alive_session::{AliveLink,AliveAgent,AliveResult},alive_io::AliveIo,native_receiver::NativeReceiverPolicy,
    rpc_pending::RpcConnectionScope,session::AttemptOwner,transport_packets::CallKind};
use std::{sync::Arc,time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use tokio::task::JoinHandle;
#[derive(Clone,Copy)]
pub(super) struct AliveBudgets{pub connect:Duration,pub rpc:Duration,pub partial:Duration,pub io:Duration}
pub(super) struct AliveRun{pub link:Arc<AliveLink>,stop:RpcConnectionScope,task:Option<JoinHandle<()>>}
impl AliveRun{
    pub(super) fn start(link:Arc<AliveLink>,owner:&AttemptOwner,main:&RpcConnectionScope,reference:&[u8],policy:&AlivePolicy,receiver:Arc<NativeReceiverPolicy>,budgets:AliveBudgets)->Result<Self,&'static str>{
        link.claim(owner,main,policy.interval)?;let stop=RpcConnectionScope::fresh();
        if policy.interval==0{link.idle("disabled");return Ok(Self{link,stop,task:None});}
        if !policy.allow_legacy_stcp{link.idle("blocked");return Ok(Self{link,stop,task:None});}
        let endpoint=match policy.select(reference){Ok(value)=>value,Err(_)=>{link.idle("unapproved");return Ok(Self{link,stop,task:None});}};
        let current=link.clone();let cancelled=stop.clone();
        let task=tokio::spawn(async move{run(current,cancelled,endpoint,receiver,budgets).await;});
        Ok(Self{link,stop,task:Some(task)})
    }
    pub(super) async fn close(&mut self)->Result<(),&'static str>{
        self.stop.close();self.link.close();
        let joined=if let Some(task)=self.task.as_mut(){task.await.map_err(|_|"app_account_alive_cleanup_unconfirmed")}else{Ok(())};
        // Commit consumption even when Join returns a panic. Cancellation
        // BEFORE it returns still retains this original, unfinished handle.
        self.task=None;joined
    }
    #[cfg(test)]
    pub(super) fn with_task_for_test(link:Arc<AliveLink>,task:JoinHandle<()>)->Self{
        Self{link,stop:RpcConnectionScope::fresh(),task:Some(task)}
    }
}
impl Drop for AliveRun{fn drop(&mut self){self.stop.close();self.link.close();}}
struct Connection{agent:AliveAgent,io:AliveIo}
fn wall_ms()->Result<u64,&'static str>{SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d|u64::try_from(d.as_millis()).ok()).ok_or("app_account_alive_clock_invalid")}
fn end(now:Instant,budget:Duration)->Result<Instant,&'static str>{now.checked_add(budget).ok_or("app_account_alive_clock_invalid")}
#[derive(Clone,Copy)]
pub(super) struct AliveBackoff(Duration);
impl AliveBackoff{
    pub(super) fn new()->Self{Self(Duration::from_secs(6))}
    pub(super) fn reset(&mut self){self.0=Duration::from_secs(6);}
    pub(super) fn fail(&mut self,now:Instant)->Result<Instant,&'static str>{let next=end(now,self.0)?;self.0=(self.0*2).min(Duration::from_secs(600));Ok(next)}
}
async fn open(link:&Arc<AliveLink>,endpoint:&super::alive_profile::AccountAliveEndpoint,budgets:AliveBudgets)->Result<Connection,&'static str>{
    let mut agent=link.agent()?;let now=Instant::now();let deadline=end(now,budgets.rpc)?;
    let request=agent.prepare(wall_ms()?,now,deadline)?;
    let mut io=AliveIo::connect(endpoint,link.main_scope(),agent.scope(),deadline.min(end(now,budgets.connect)?),budgets.partial,budgets.io).await?;
    io.send(CallKind::Request,request.expose_to_transport(),deadline).await?;Ok(Connection{agent,io})
}
async fn run(link:Arc<AliveLink>,stop:RpcConnectionScope,endpoint:super::alive_profile::AccountAliveEndpoint,receiver:Arc<NativeReceiverPolicy>,budgets:AliveBudgets){
    let main=link.main_scope();let mut connection:Option<Connection>=None;let mut wake=Instant::now();let mut backoff=AliveBackoff::new();
    loop{
        if !stop.is_open()||!link.live(){break;}
        if connection.is_none(){
            if Instant::now()<wake{tokio::select!{biased;_=stop.closed()=>break,_=main.session_ended()=>break,_=tokio::time::sleep_until(wake.into())=>{}}}
            if !stop.is_open()||!link.live(){break;}
            match open(&link,&endpoint,budgets).await{
                Ok(value)=>connection=Some(value),
                Err(_)=>{if !stop.is_open()||!link.live(){break;}link.idle("retrying");let Ok(next)=backoff.fail(Instant::now())else{break;};wake=next;continue;}
            }
        }
        let Some(current)=connection.as_mut()else{break;};let now=Instant::now();
        let step=async{
            if current.agent.pending_deadline().is_none(){
                if link.label()=="registered"{wake=link.due(now)?;}
                if now>=wake{
                    let deadline=end(now,budgets.rpc)?;let request=current.agent.prepare(wall_ms()?,now,deadline)?;
                    current.io.send(CallKind::Request,request.expose_to_transport(),deadline).await?;
                }
            }
            let deadline=current.agent.pending_deadline().unwrap_or(wake);
            if current.agent.pending_deadline().is_some_and(|deadline|Instant::now()>=deadline){return Err("app_account_alive_timeout");}
            let Some(messages)=current.io.receive(deadline).await? else{return Ok(false);};
            // Validate the whole chunk before a positive registration or callback
            // side effect can escape; a later malformed command invalidates it.
            for message in &messages{current.agent.validate_incoming(message.kind,message.payload.expose_to_transport(),&receiver)?;}
            for message in messages{
                if Instant::now()>=message.deadline{return Err("app_account_alive_timeout");}
                match message.kind{
                    CallKind::Reply=>match current.agent.response(message.payload.expose_to_transport(),Instant::now())?{
                        Some(AliveResult::Accepted)=>{backoff.reset();wake=link.due(Instant::now())?;}
                        Some(AliveResult::Rejected)=>wake=backoff.fail(Instant::now())?,
                        Some(AliveResult::SessionInvalid)=>return Ok(true),None=>{},
                    },
                    CallKind::Request=>{
                        let reply=current.agent.callback(message.payload.expose_to_transport(),Instant::now(),&receiver)?;
                        let deadline=current.agent.pending_deadline().map_or(message.deadline,|old|old.min(message.deadline));
                        current.io.send(CallKind::Reply,reply.expose_to_transport(),deadline).await?;
                    },
                    _=>return Err("app_account_alive_protocol_invalid"),
                }
            }Ok(false)
        }.await;
        match step{
            Ok(true)=>break,Ok(false)=>{},Err(_)=>{
                connection=None;if !stop.is_open()||!link.live(){break;}link.idle("retrying");let Ok(next)=backoff.fail(Instant::now())else{break;};wake=next;
            }
        }
    }
    drop(connection);
}
