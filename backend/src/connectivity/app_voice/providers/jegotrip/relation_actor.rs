//! Original actor work for independent relation WRITEs, not a detached worker.
use super::*;
use super::super::{arc_session::RelationIdentity,relation_journal::{RelationPermit,RelationPermitWait,RelationWriteView}};
pub(super) struct RelationWork{pub(super) permit:Arc<RelationPermit>,pub(super) reply:oneshot::Sender<Result<RelationWriteView,RuntimeError>>}
pub(super) struct RelationPending{pub(super) permit:Arc<RelationPermit>,pub(super) deadline:Instant,pub(super) reply:oneshot::Sender<Result<RelationWriteView,RuntimeError>>}
impl RuntimeCommands{
    pub(in super::super) async fn original_relation_identity(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Result<Arc<RelationIdentity>,RuntimeError>{
        if !self.accepts(owner,scope){return Err(RuntimeError::Scope);}let(reply,data)=oneshot::channel();self.submit(owner,scope,Operation::GetRelationIdentity(reply)).await?;data.await.map_err(|_|RuntimeError::Closed)?.ok_or(RuntimeError::Scope)
    }
    pub(in super::super) async fn update_relations(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,permit:Arc<RelationPermit>)->Result<RelationWriteView,RuntimeError>{
        let _wait=RelationPermitWait::original(permit.clone());if !self.accepts(owner,scope)||!permit.matches(owner,scope){return Err(RuntimeError::Scope);}
        let(reply,data)=oneshot::channel();let submitted=self.enqueue(owner,scope,Operation::RelationUpdate(Box::new(RelationWork{permit,reply})))?;submitted.await.map_err(|_|RuntimeError::Closed)??;data.await.map_err(|_|RuntimeError::Closed)?
    }
}
impl<I:AsyncRead+AsyncWrite+Unpin+Send+'static> Actor<I>{
    pub(super) fn relation_close_reason(&self)->Option<RuntimeError>{self.relation_retirement.or_else(||self.relation_pending.values().find_map(|p|p.permit.closed_original_io().then_some(RuntimeError::Cancelled)))}
    pub(super) fn retire_relation_updates(&mut self,now:Instant){
        let ended=self.relation_pending.iter().filter_map(|(&id,p)|{
            if p.permit.retired()||p.reply.is_closed()||!p.permit.gate().live()||!p.permit.parent_gate().live(){Some((id,RuntimeError::Cancelled))}else if now>=p.deadline{Some((id,RuntimeError::Timeout))}else{None}
        }).collect::<Vec<_>>();
        for(id,error)in ended{if let Some(p)=self.relation_pending.remove(&id){p.permit.cancel();if p.permit.closed_original_io(){self.relation_retirement.get_or_insert(error);}self.backend.retire_relation_update(id);self.pending.remove(&id);let _=p.reply.send(Err(error));}}
    }
    pub(super) fn deliver_relation_replies(&mut self,outcome:Result<(),RuntimeError>)->Result<(),RuntimeError>{
        for reply in self.backend.take_relation_replies(){
            let id=reply.request().id();let Some(p)=self.relation_pending.remove(&id)else{continue;};self.pending.remove(&id);
            if outcome.is_err()||!reply.matches(&self.owner,&self.scope)||!Arc::ptr_eq(reply.request().permit(),&p.permit)||Instant::now()>=p.deadline||p.reply.is_closed(){p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Cancelled));return Err(RuntimeError::Cancelled);}
            match p.permit.resolve(&reply){Ok(view)=>{let _=p.reply.send(Ok(view));},Err(_)=>{p.permit.cancel();let _=p.reply.send(Err(RuntimeError::Protocol));return Err(RuntimeError::Protocol);}}
        }Ok(())
    }
}
