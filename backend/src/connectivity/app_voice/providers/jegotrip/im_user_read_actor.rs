//! Original native binding lookup and user-requested mark-read WRITE. No
//! token/UID is rebuilt from credentials; no received/read reply is converted.
use super::*;
use super::super::{im_delivery::ImBinding,im_inbox::{UserReadPermit,UserReadPermitWait,UserReadView}};
pub(super) struct UserReadWork{pub(super) permit:Arc<UserReadPermit>,pub(super) reply:oneshot::Sender<Result<UserReadView,RuntimeError>>}
pub(super) struct UserReadPending{pub(super) permit:Arc<UserReadPermit>,pub(super) deadline:Instant,pub(super) reply:oneshot::Sender<Result<UserReadView,RuntimeError>>}
impl RuntimeCommands{
    pub(in super::super) async fn original_im_binding(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Result<Arc<ImBinding>,RuntimeError>{
        if !self.accepts(owner,scope){return Err(RuntimeError::Scope);}let(reply,result)=oneshot::channel();
        self.submit(owner,scope,Operation::GetImBinding(reply)).await?;result.await.map_err(|_|RuntimeError::Closed)?.ok_or(RuntimeError::Scope)
    }
    pub(in super::super) async fn read_user_target(&self,action:&super::super::im_inbox::UserReadAction,query:Arc<ImQuery>)->Result<OwnedImReply,RuntimeError>{
        let scope=action.scope();let gate=action.gate();if !self.accepts(action.owner(),&scope)||!action.live()||query.operation()!=ImOperation::Refresh{return Err(RuntimeError::Scope);}
        let interest=ImReadInterest::original(&scope);let(reply,result)=oneshot::channel();let _wait=ImReadWait::original(interest.clone());
        let complete=self.enqueue(action.owner(),&scope,Operation::ImRead(Box::new(ImReadWork{query,gate:gate.clone(),interest,reply})))?;
        complete.await.map_err(|_|RuntimeError::Closed)??;
        #[cfg(test)]
        action.note_target_submitted();
        let result=result.await.map_err(|_|RuntimeError::Closed)??;if !result.matches(action.owner(),&scope)||!gate.live()||!action.live(){return Err(RuntimeError::Scope);}Ok(result)
    }
    pub(in super::super) async fn user_read_write(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,permit:Arc<UserReadPermit>)->Result<UserReadView,RuntimeError>{
        let _wait=UserReadPermitWait::original(permit.clone());if !self.accepts(owner,scope)||!permit.matches(owner,scope){return Err(RuntimeError::Scope);}
        let(reply,result)=oneshot::channel();let submitted=self.enqueue(owner,scope,Operation::UserMarkRead(Box::new(UserReadWork{permit,reply})))?;
        submitted.await.map_err(|_|RuntimeError::Closed)??;result.await.map_err(|_|RuntimeError::Closed)?
    }
}
impl<I:AsyncRead+AsyncWrite+Unpin+Send+'static> Actor<I>{
    /// Preserve the original WRITE retirement cause across a generic scope
    /// closed exit; no past queued/read cancellation or non-close error maps.
    pub(super) fn user_write_close_reason(&self)->Option<RuntimeError>{
        self.user_write_retirement.or_else(||self.user_read_pending.values().find_map(|p|p.permit.closed_original_io().then_some(RuntimeError::Cancelled)))
    }
    pub(super) fn retire_user_read_writes(&mut self,now:Instant){
        let ids=self.user_read_pending.iter().filter_map(|(&id,p)|{if p.permit.retired()||p.reply.is_closed()||!p.permit.gate().live()||!p.permit.parent_gate().live(){Some((id,RuntimeError::Cancelled))}else if now>=p.deadline{Some((id,RuntimeError::Timeout))}else{None}}).collect::<Vec<_>>();
        for(id,e)in ids{if let Some(p)=self.user_read_pending.remove(&id){p.permit.cancel();if p.permit.closed_original_io(){self.user_write_retirement.get_or_insert(e);}self.backend.retire_user_read_write(id);self.pending.remove(&id);let _=p.reply.send(Err(e));}}
    }
    pub(super) fn deliver_user_read_replies(&mut self,outcome:Result<(),RuntimeError>)->Result<(),RuntimeError>{
        for r in self.backend.take_user_read_replies(){let id=r.request().id();let Some(p)=self.user_read_pending.remove(&id)else{continue;};self.pending.remove(&id);
            if outcome.is_err()||!r.matches(&self.owner,&self.scope)||!Arc::ptr_eq(r.request().permit(),&p.permit)||Instant::now()>=p.deadline||p.reply.is_closed(){p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Cancelled));return Err(RuntimeError::Cancelled);}
            match p.permit.resolve(&r){Ok(view)=>{let _=p.reply.send(Ok(view));},Err(_)=>{p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Protocol));return Err(RuntimeError::Protocol);}}
        }Ok(())
    }
}
