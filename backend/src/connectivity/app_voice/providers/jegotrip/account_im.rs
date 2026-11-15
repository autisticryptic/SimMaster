//! Original-run Message IM consumer. Shares the existing owned account event
//! task/Join, never an SDK/phone process, SMS projection or detached worker.
use super::*;
use super::super::{im_delivery::IncomingImDelivery,im_inbox::{ImInboxScope,CommittedIm,AssociationAdmission,ImPendingAssociation}};
use std::collections::BTreeMap;
const MAX_GROUPS:usize=64;
const MAX_HOLDERS:usize=256;
const MAX_BYTES:usize=1024*1024;
const MAX_EARLY:usize=16;
const INVALID:&str="app_account_im_scope_invalid";
const FULL:&str="app_account_im_backpressure";
struct Group{fingerprint:[u8;32],key:[u8;32],deadline:Instant,context:ImInboxScope,pending:Option<ImPendingAssociation>,holders:VecDeque<Box<IncomingImDelivery>>,outcome:Option<Option<CommittedIm>>}
pub(super) struct State{early:VecDeque<Box<IncomingImDelivery>>,early_bytes:usize,ordered:VecDeque<Box<IncomingImDelivery>>,groups:BTreeMap<u64,Group>,holders:usize,bytes:usize}
impl Default for State{fn default()->Self{Self{early:VecDeque::new(),early_bytes:0,ordered:VecDeque::new(),groups:BTreeMap::new(),holders:0,bytes:0}}}
impl State{pub(super) fn clear(&mut self){self.early.clear();self.early_bytes=0;self.ordered.clear();self.groups.clear();self.holders=0;self.bytes=0;}}
fn context(run:&ManagedRun,delivery:&IncomingImDelivery)->Result<ImInboxScope,&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.im_messages_enabled(){return Err(INVALID);}
    run.configuration_current()?;
    ImInboxScope::original(run.run_token.clone(),delivery.notice().binding().clone(),run.im_gate.clone())
}
async fn complete(run:&ManagedRun,delivery:&IncomingImDelivery,proof:Option<&CommittedIm>,deadline:Instant)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){return Err(INVALID);}
    run.configuration_current()?;
    let ticket=delivery.ticket().clipped_to(deadline);
    let proof=proof.filter(|proof|proof.authorizes(&ticket,Instant::now()));
    let receipt=run.backend.complete_im(delivery.owner(),&delivery.scope(),&ticket,proof).await.map_err(|_|INVALID)?;
    if !receipt.matches(delivery.owner(),&delivery.scope()){return Err(INVALID);}Ok(())
}
fn settle(run:&ManagedRun,group:&mut Group,proof:Option<&CommittedIm>)->Result<(),&'static str>{
    if group.outcome.is_some(){return Ok(());}
    let pending=group.pending.as_ref().ok_or(INVALID)?;
    let result=group.context.finish_association(&run.database,pending,proof)?;
    group.pending=None;group.outcome=Some(result);Ok(())
}
async fn drain(run:&ManagedRun,state:&mut State)->Result<(),&'static str>{
    for group in state.groups.values_mut(){
        if Instant::now()>=group.deadline&&group.outcome.is_none(){settle(run,group,None)?;}
        if group.outcome.is_none(){continue;}
        // The data/ticket remains charged while its actual completion awaits.
        while let Some(delivery)=group.holders.front(){
            let proof=group.outcome.as_ref().and_then(|value|value.as_ref());
            // 581648 answers a duplicate true when the original holder was
            // already consumed. That is replay of a proven result, not an
            // extension/revival of an expired UNCONSUMED notification. Each
            // original RPC still has its own unchanged response deadline.
            let deadline=if proof.is_some(){delivery.ticket().deadline()}else{group.deadline};
            complete(run,delivery,proof,deadline).await?;
            let bytes=delivery.bytes();group.holders.pop_front();
            state.holders=state.holders.checked_sub(1).ok_or(INVALID)?;state.bytes=state.bytes.checked_sub(bytes).ok_or(INVALID)?;
        }
    }
    // Completed associations stay in bounded private SQLite, not this live
    // budget. The actor separately retains BatchQueued proofs until the exact
    // peer epoch ACK. Removing a queue entry is never transport retirement.
    state.groups.retain(|_,group|group.outcome.is_none()||!group.holders.is_empty());
    Ok(())
}
async fn consume(run:&ManagedRun,state:&mut State,delivery:Box<IncomingImDelivery>)->Result<(),&'static str>{
    let scope=context(run,&delivery)?;
    if run.backend.im_sync_enabled()&&delivery.notice().notification_id().is_none(){
        // Retain the SAME original holder and first deadline while current
        // refresh/gap IO is pending. All bytes/inflight share the old budget.
        let bytes=state.bytes.checked_add(delivery.bytes()).ok_or(FULL)?;
        if state.holders>=MAX_HOLDERS||bytes>MAX_BYTES{return Err(FULL);}
        state.ordered.push_back(delivery);state.holders+=1;state.bytes=bytes;
        #[cfg(test)]
        run.im_original_queue_admissions.fetch_add(1,Ordering::SeqCst);
        return drain(run,state).await;
    }
    if let Some(id)=delivery.notice().notification_id(){
        let bytes=delivery.bytes();let total=state.bytes.checked_add(bytes).ok_or(FULL)?;
        if state.holders>=MAX_HOLDERS||total>MAX_BYTES{return Err(FULL);}
        if let Some(group)=state.groups.get(&id){
            if group.fingerprint!=delivery.notice().fingerprint()||group.key!=*delivery.notice().key().digest(){return Err(INVALID);}
        }else{
            if state.groups.len()>=MAX_GROUPS{return Err(FULL);}
            let (pending,outcome)=match scope.open_association(&run.database,&delivery)?{
                AssociationAdmission::Fresh(pending)=>{
                    let expired=Instant::now()>=delivery.ticket().deadline();
                    let proof=if expired{None}else{scope.lookup(&run.database,&delivery)?};
                    if expired||proof.is_some(){
                        let result=scope.finish_association(&run.database,&pending,proof.as_ref())?;(None,Some(result))
                    }else{(Some(pending),None)}
                },
                AssociationAdmission::Replay(proof)=>(None,Some(proof)),
            };
            state.groups.insert(id,Group{fingerprint:delivery.notice().fingerprint(),key:*delivery.notice().key().digest(),deadline:delivery.ticket().deadline(),context:scope,pending,holders:VecDeque::new(),outcome});
        }
        state.groups.get_mut(&id).ok_or(INVALID)?.holders.push_back(delivery);state.holders+=1;state.bytes=total;
        drain(run,state).await
    }else{
        if Instant::now()>=delivery.ticket().deadline(){return complete(run,&delivery,None,delivery.ticket().deadline()).await;}
        let result=scope.commit(&run.database,&delivery);
        let proof=match result{Ok(proof)=>proof,Err(code)=>{*lock(&run.error)=Some(code);None}};
        complete(run,&delivery,proof.as_ref(),delivery.ticket().deadline()).await?;
        if let Some(proof)=proof{
            for group in state.groups.values_mut(){
                if group.outcome.is_none()&&group.key==*delivery.notice().key().digest(){
                    settle(run,group,Some(&proof))?;
                }
            }
        }
        drain(run,state).await
    }
}
/// Early native IM waits for the SAME original HostingAck under its received
/// deadline. Neither a notification nor a parsed body can create ready/ACK.
pub(super) async fn delivery(run:&ManagedRun,phase:Phase,state:&mut State,delivery:Box<IncomingImDelivery>)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.backend.im_messages_enabled(){return Err(INVALID);}
    if phase==Phase::HostingPending{
        let bytes=state.early_bytes.checked_add(delivery.bytes()).ok_or(FULL)?;
        if state.early.len()>=MAX_EARLY||bytes>MAX_BYTES{return Err(FULL);}
        state.early.push_back(delivery);state.early_bytes=bytes;return Ok(());
    }
    if phase!=Phase::Hosted{return complete(run,&delivery,None,delivery.ticket().deadline()).await;}
    consume(run,state,delivery).await
}
pub(super) async fn drive(run:&ManagedRun,phase:Phase,state:&mut State)->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){state.clear();return Ok(());}
    if phase!=Phase::Hosted{return Ok(());}
    while let Some(delivery)=state.early.pop_front(){
        let bytes=delivery.bytes();consume(run,state,delivery).await?;state.early_bytes=state.early_bytes.checked_sub(bytes).ok_or(INVALID)?;
    }
    drain(run,state).await
}
/// Read records settle only matching already-admitted original notifications.
/// A gap/absence/cursor by itself never acknowledges an unrelated holder.
pub(super) async fn synchronized(run:&ManagedRun,state:&mut State,proofs:&[CommittedIm])->Result<(),&'static str>{
    if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){return Err(INVALID);}
    run.configuration_current()?;
    for proof in proofs{for group in state.groups.values_mut(){
        if group.outcome.is_none()&&group.key==*proof.key_digest(){settle(run,group,Some(proof))?;}
    }}
    drain(run,state).await
}
fn release_ordered(state:&mut State,bytes:usize)->Result<(),&'static str>{
    state.holders=state.holders.checked_sub(1).ok_or(INVALID)?;state.bytes=state.bytes.checked_sub(bytes).ok_or(INVALID)?;Ok(())
}
/// Rotate a bounded current queue once. A receive in flight may observe new
/// targets but cannot be overlapped with another cursor advance for that box.
pub(super) async fn ordered(run:&ManagedRun,state:&mut State,cursor:Option<&super::super::im_inbox::CursorSession>,blocked_box:Option<&[u8]>)->Result<(),&'static str>{
    use super::super::im_inbox::OnlinePosition;
    let count=state.ordered.len();
    for _ in 0..count{
        let delivery=state.ordered.pop_front().ok_or(INVALID)?;let bytes=delivery.bytes();
        if Instant::now()>=delivery.ticket().deadline(){
            let sent=complete(run,&delivery,None,delivery.ticket().deadline()).await;
            if let Err(code)=sent{state.ordered.push_front(delivery);return Err(code);}
            drop(delivery);release_ordered(state,bytes)?;continue;
        }
        let Some(cursor)=cursor else{state.ordered.push_back(delivery);continue;};
        let position=match cursor.online_position(&run.database,&delivery){Ok(p)=>p,Err(code)=>{state.ordered.push_front(delivery);return Err(code);}};
        if matches!(position,OnlinePosition::Pending)||matches!(position,OnlinePosition::Next)&&blocked_box==Some(delivery.notice().key().box_id().bytes()){
            state.ordered.push_back(delivery);continue;
        }
        let result=if matches!(position,OnlinePosition::MissingOlder){Ok(None)}else{cursor.commit_online(&run.database,&delivery)};
        let proof=match result{Ok(proof)=>proof,Err(code)=>{
            let sent=complete(run,&delivery,None,delivery.ticket().deadline()).await;
            if sent.is_err(){state.ordered.push_front(delivery);return Err(INVALID);}
            drop(delivery);release_ordered(state,bytes)?;return Err(code);
        }};
        let sent=complete(run,&delivery,proof.as_ref(),delivery.ticket().deadline()).await;
        if let Err(code)=sent{state.ordered.push_front(delivery);return Err(code);}
        drop(delivery);release_ordered(state,bytes)?;
        if let Some(proof)=proof{synchronized(run,state,std::slice::from_ref(&proof)).await?;}
    }
    Ok(())
}
