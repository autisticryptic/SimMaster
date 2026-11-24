//! Shared original positive RPC allocation for independently authorized relation WRITE.
use super::*;
use super::super::{relation_journal::RelationPermit,rpc_relation_update::{self,RelationRpcRequest,OwnedRelationReply}};
impl ArcRpcSession{
    pub(in super::super) fn prepare_relation_update(&mut self,binding:Arc<super::super::im_delivery::ImBinding>,permit:Arc<RelationPermit>,deadline:std::time::Instant)->Result<(i32,RpcBytes),CodecError>{
        if !self.native_receiver_scope()||!binding.matches(&self.owner,&self.connection)||!Arc::ptr_eq(&binding,permit.binding())||!permit.matches(&self.owner,&self.connection)||std::time::Instant::now()>=deadline.min(permit.deadline()){return Err(CodecError::InvalidField);}
        if self.pending.len()>=MAX_MAP_ENTRIES||self.im_reply_count()>=MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        if !binding.authenticates(&auth.user_id,&auth.selected_session_id){return Err(CodecError::InvalidField);}
        let id=self.next_id.ok_or(CodecError::InvalidCallId)?;
        let(request,bytes)=rpc_relation_update::encode_original(permit,id,&self.owner,&self.connection,&auth.token,deadline)?;
        self.next_id=id.checked_add(1);self.relation_requests.insert(id,request);
        self.pending.insert(id,PendingRpc{kind:PendingKind::RelationUpdate,selected_session_id:None,scheduler:None,call:None,im:None,received:None,user_read:None,send:None});Ok((id,bytes))
    }
    pub(in super::super) fn pending_relation_update(&self,id:i32)->Option<Arc<RelationRpcRequest>>{self.relation_requests.get(&id).cloned()}
    pub(in super::super) fn retire_relation_update(&mut self,id:i32){
        if self.pending.get(&id).is_some_and(|p|matches!(p.kind,PendingKind::RelationUpdate)){self.pending.remove(&id);}
        self.relation_requests.remove(&id);
    }
    pub(in super::super) fn take_relation_replies(&mut self)->Vec<OwnedRelationReply>{std::mem::take(&mut self.relation_replies)}
    pub(super) fn captured_relation_reply(&mut self,id:i32,owner:&AttemptOwner,scope:&RpcConnectionScope,code:u32,bytes:&[u8])->Result<(),CodecError>{
        let request=self.relation_requests.remove(&id).ok_or(CodecError::InvalidField)?;
        if self.phase()!=ArcPhase::Authenticated||!request.matches(owner,scope){self.close();return Err(CodecError::InvalidField);}
        let reply=match OwnedRelationReply::original(request,owner,scope,code,bytes){Ok(r)=>r,Err(e)=>{self.close();return Err(e);}};
        if self.im_reply_count()>=MAX_MAP_ENTRIES||self.im_reply_bytes().checked_add(reply.retained_bytes()).is_none_or(|n|n>super::super::rpc_stream::MAX_STREAM_BYTES){self.close();return Err(CodecError::TooLarge);}
        self.relation_replies.push(reply);Ok(())
    }
}
