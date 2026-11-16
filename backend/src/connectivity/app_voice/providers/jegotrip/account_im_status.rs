//! Status.Update uses the original account consumer and shared IM budget. No
//! separate task, storage-derived message ACK, automatic query or remote write.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{im_status_delivery::IncomingImStatus,im_inbox::CommittedImStatus};
async fn complete(run:&ManagedRun,delivery:&IncomingImStatus,proof:Option<&CommittedImStatus>)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){return Err(INVALID);}run.configuration_current()?;
    let proof=proof.filter(|proof|proof.authorizes(delivery.ticket(),Instant::now()));
    let receipt=run.backend.complete_im_status(delivery.owner(),&delivery.scope(),delivery.ticket(),proof).await.map_err(|_|INVALID)?;
    if !receipt.matches(delivery.owner(),&delivery.scope()){return Err(INVALID);}Ok(())
}
async fn consume(run:&ManagedRun,state:&mut State,delivery:Box<IncomingImStatus>)->Result<(),&'static str>{
    let bytes=delivery.bytes();let total=state.bytes.checked_add(bytes).ok_or(FULL)?;
    if state.holders>=MAX_HOLDERS||total>MAX_BYTES{return Err(FULL);}state.holders+=1;state.bytes=total;
    let proof=if Instant::now()>=delivery.ticket().deadline(){None}else{
        run.configuration_current()?;
        let context=ImInboxScope::original(run.run_token.clone(),delivery.notice().binding().clone(),run.im_gate.clone())?;
        match context.commit_status(&run.database,&delivery){Ok(proof)=>Some(proof),Err(code)=>{*lock(&run.error)=Some(code);None}}
    };
    let result=complete(run,&delivery,proof.as_ref()).await;
    // The original holder stays charged through its actual awaited completion.
    drop(delivery);state.holders=state.holders.checked_sub(1).ok_or(INVALID)?;state.bytes=state.bytes.checked_sub(bytes).ok_or(INVALID)?;result
}
pub(in super::super) async fn delivery(run:&ManagedRun,phase:Phase,state:&mut State,delivery:Box<IncomingImStatus>)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.im_messages_enabled(){return Err(INVALID);}
    if phase==Phase::HostingPending{
        let bytes=state.early_bytes.checked_add(delivery.bytes()).ok_or(FULL)?;
        if state.early.len()+state.early_status.len()>=MAX_EARLY||bytes>MAX_BYTES{return Err(FULL);}
        state.early_status.push_back(delivery);state.early_bytes=bytes;return Ok(());
    }
    if phase!=Phase::Hosted{return complete(run,&delivery,None).await;}
    consume(run,state,delivery).await
}
pub(super) async fn drive(run:&ManagedRun,state:&mut State)->Result<(),&'static str>{
    while let Some(delivery)=state.early_status.pop_front(){
        let bytes=delivery.bytes();consume(run,state,delivery).await?;state.early_bytes=state.early_bytes.checked_sub(bytes).ok_or(INVALID)?;
    }Ok(())
}
