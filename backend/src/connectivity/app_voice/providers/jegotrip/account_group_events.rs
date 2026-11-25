//! Incoming Group.Update uses the original account event task and shared early
//! receiver queue/byte limit, not a new SDK task or relation WRITE reconciliation.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{group_event_delivery::IncomingGroupEvent,group_event_inbox::{self,CommittedGroupEvent}};
type EventFuture<'a>=futures_util::future::BoxFuture<'a,Result<(),&'static str>>;
async fn complete(run:&ManagedRun,event:&IncomingGroupEvent,proof:Option<&CommittedGroupEvent>)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){return Err(INVALID);}run.configuration_current()?;
    let configuration=run.relation_configuration.as_ref().ok_or(INVALID)?;
    run.database.with_conn(|c|configuration.require_current(c,run.run_token.line_id(),run.run_token.revision(),&event.scope()))?;
    let proof=proof.filter(|p|p.authorizes(event.ticket(),Instant::now()));
    let receipt=run.backend.complete_group_event(event.owner(),&event.scope(),event.ticket(),proof).await.map_err(|_|INVALID)?;
    if !receipt.matches(event.owner(),&event.scope()){return Err(INVALID);}Ok(())
}
async fn consume(run:&ManagedRun,state:&mut State,event:Box<IncomingGroupEvent>)->Result<(),&'static str>{
    let configuration=run.relation_configuration.as_ref().ok_or(INVALID)?;
    let bytes=event.bytes();let total=state.bytes.checked_add(bytes).ok_or(FULL)?;if state.holders>=MAX_HOLDERS||total>MAX_BYTES{return Err(FULL);}state.holders+=1;state.bytes=total;
    let proof=if Instant::now()>=event.ticket().deadline(){None}else{
        match group_event_inbox::commit(&run.database,run.run_token.clone(),&event,run.im_gate.clone(),configuration){Ok(p)=>Some(p),Err(code)=>{*lock(&run.error)=Some(code);None}}
    };
    let result=complete(run,&event,proof.as_ref()).await;drop(event);state.holders=state.holders.checked_sub(1).ok_or(INVALID)?;state.bytes=state.bytes.checked_sub(bytes).ok_or(INVALID)?;result
}
pub(in super::super) fn delivery<'a>(run:&'a ManagedRun,phase:Phase,state:&'a mut State,event:Box<IncomingGroupEvent>)->EventFuture<'a>{
    Box::pin(async move{
        if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.relation_events_enabled()||!run.backend.original_scope(event.owner()).is_some_and(|scope|scope.matches(&event.scope())){return Err(INVALID);}
        if phase==Phase::HostingPending{
            let bytes=state.early_bytes.checked_add(event.bytes()).ok_or(FULL)?;
            if state.early.len()+state.early_status.len()+state.early_group_events.len()>=MAX_EARLY||bytes>MAX_BYTES{return Err(FULL);}
            state.early_group_events.push_back(event);state.early_bytes=bytes;return Ok(());
        }
        if phase!=Phase::Hosted{return complete(run,&event,None).await;}consume(run,state,event).await
    })
}
pub(super) fn drive<'a>(run:&'a ManagedRun,state:&'a mut State)->EventFuture<'a>{
    if state.early_group_events.is_empty(){return Box::pin(std::future::ready(Ok(())));}
    Box::pin(async move{while let Some(event)=state.early_group_events.pop_front(){let bytes=event.bytes();consume(run,state,event).await?;state.early_bytes=state.early_bytes.checked_sub(bytes).ok_or(INVALID)?;}Ok(())})
}
