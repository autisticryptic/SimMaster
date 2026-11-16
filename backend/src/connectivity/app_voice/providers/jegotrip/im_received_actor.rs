//! Original received WRITE actor boundary (own Actions still required). No caller raw query,
//! new task, queued replay or readonly submitted-interest cancellation rule.
use super::*;
use super::super::{im_inbox::{ReceivedWritePermit,ReceivedWait},rpc_im_received::OwnedReceivedReply};
pub(super) struct ReceivedWork{pub(super) permit:Arc<ReceivedWritePermit>,pub(super) reply:oneshot::Sender<Result<OwnedReceivedReply,RuntimeError>>}
pub(super) struct ReceivedPending{pub(super) permit:Arc<ReceivedWritePermit>,pub(super) deadline:Instant,pub(super) reply:oneshot::Sender<Result<OwnedReceivedReply,RuntimeError>>}
impl RuntimeCommands{
    pub(in super::super) async fn write_received(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,permit:Arc<ReceivedWritePermit>)->Result<OwnedReceivedReply,RuntimeError>{
        // The durable marker predates admission. Even a rejected local queue
        // owns a waiter Drop so its known no-IO cancellation can be recorded.
        let _wait=ReceivedWait::original(permit.clone());
        if !self.accepts(owner,scope)||!permit.matches(owner,scope){return Err(RuntimeError::Scope);}
        let(reply,result)=oneshot::channel();
        let sent=self.enqueue(owner,scope,Operation::WriteReceived(Box::new(ReceivedWork{permit,reply})))?;
        sent.await.map_err(|_|RuntimeError::Closed)??;
        let result=result.await.map_err(|_|RuntimeError::Closed)??;
        if !result.matches(owner,scope){return Err(RuntimeError::Scope);}Ok(result)
    }
}
impl<I:AsyncRead+AsyncWrite+Unpin+Send+'static> Actor<I>{
    pub(super) fn retire_received_writes(&mut self,now:Instant){
        let retired=self.received_pending.iter().filter_map(|(&id,p)|{
            if p.permit.is_retired()||p.reply.is_closed()||!p.permit.gate().live(){Some((id,RuntimeError::Cancelled))}
            else if now>=p.deadline{Some((id,RuntimeError::Timeout))}else{None}
        }).collect::<Vec<_>>();
        for(id,reason)in retired{if let Some(p)=self.received_pending.remove(&id){
            // Submitted remote writes remain unknown and close this original
            // scope. Removing only a readonly reply interest is not sufficient.
            p.permit.cancel();self.backend.retire_received_write(id);self.pending.remove(&id);let _=p.reply.send(Err(reason));
        }}
    }
    pub(super) fn deliver_received_replies(&mut self,outcome:Result<(),RuntimeError>)->Result<(),RuntimeError>{
        for reply in self.backend.take_received_replies(){
            let id=reply.request().id();let Some(p)=self.received_pending.remove(&id)else{continue;};self.pending.remove(&id);
            if outcome.is_err()||!self.scope.is_open()||self.scope.session_revoked()||p.reply.is_closed()||Instant::now()>=p.deadline
                ||!reply.matches(&self.owner,&self.scope)||!Arc::ptr_eq(reply.request().permit(),&p.permit){
                p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Cancelled));return Err(RuntimeError::Cancelled);
            }
            // Whole original reply parsing has completed. SQL journal CAS is
            // outside the nonblocking physical write gate, not an IO grant.
            if p.permit.resolve_original(&reply).is_err(){p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Protocol));return Err(RuntimeError::Protocol);}
            let _=p.reply.send(Ok(reply));
        }Ok(())
    }
}
