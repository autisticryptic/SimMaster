//! Original ARC received WRITE allocation/reply handling (own CI still needed).
//! Separate from prepare_im_read and late readonly interest retirement.
use super::*;
use super::super::{im_inbox::ReceivedWritePermit,rpc_im_received::{ReceivedRpcRequest,OwnedReceivedReply}};
impl ArcRpcSession{
    pub(in super::super) fn prepare_received_write(&mut self,binding:Arc<super::super::im_delivery::ImBinding>,permit:Arc<ReceivedWritePermit>,deadline:std::time::Instant)->Result<(i32,RpcBytes),CodecError>{
        let deadline=permit.effective_deadline(deadline);
        if !self.native_receiver_scope()||!binding.matches(&self.owner,&self.connection)||!permit.matches(&self.owner,&self.connection)
            ||!Arc::ptr_eq(&binding,permit.binding())||std::time::Instant::now()>=deadline{return Err(CodecError::InvalidField);}
        if self.pending.len()>=MAX_MAP_ENTRIES||self.im_reply_count()>=MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        if !binding.authenticates(&auth.user_id,&auth.selected_session_id){return Err(CodecError::InvalidField);}
        let query=permit.query();if query.operation()!=super::super::rpc_im::ImOperation::SetReceived{return Err(CodecError::InvalidField);}
        let arguments=query.arguments(&auth.user_id)?;let id=self.next_id.ok_or(CodecError::InvalidCallId)?;
        let bytes=encode_request(id,&[RpcProperty{key:b"id",value:super::super::rpc_im::GROUP_OBJECT},RpcProperty{key:b"token",value:&auth.token}],query.method().as_bytes(),arguments.expose_to_transport())?;
        let retained=bytes.expose_to_transport().len().checked_add(query.retained_bytes()).and_then(|n|n.checked_add(64)).ok_or(CodecError::TooLarge)?;
        let budget=self.connection.reserve_native_rpc(retained)?;
        permit.arm_request(id,deadline).map_err(|_|CodecError::InvalidField)?;
        // Once the durable permit names this ID it is consumed even if a
        // following local scope check fails. IDs are never reused for a retry.
        self.next_id=id.checked_add(1);
        let received=ReceivedRpcRequest::original(permit,id,budget)?;
        self.pending.insert(id,PendingRpc{kind:PendingKind::ImWrite,selected_session_id:None,scheduler:None,call:None,im:None,received:Some(received),user_read:None,send:None});Ok((id,bytes))
    }
    pub(in super::super) fn take_received_replies(&mut self)->Vec<OwnedReceivedReply>{std::mem::take(&mut self.received_replies)}
    pub(in super::super) fn retire_received_write(&mut self,id:i32){
        if self.pending.get(&id).is_some_and(|p|matches!(p.kind,PendingKind::ImWrite)){self.pending.remove(&id);}
    }
    pub(super) fn captured_received_reply(&mut self,request:Arc<ReceivedRpcRequest>,owner:&AttemptOwner,scope:&RpcConnectionScope,code:u32,bytes:&[u8])->Result<(),CodecError>{
        // A submitted write lost/expired/revoked here remains unknown. It does
        // NOT use the readonly late-interest Ok(None)/healthy SDK rule.
        if self.phase()!=ArcPhase::Authenticated||!request.matches(owner,scope){self.close();return Err(CodecError::InvalidField);}
        let reply=match OwnedReceivedReply::original(request,code,bytes){Ok(r)=>r,Err(e)=>{self.close();return Err(e);}};
        let retained=self.im_reply_bytes();
        if self.im_reply_count()>=MAX_MAP_ENTRIES||retained.checked_add(reply.retained_bytes()).is_none_or(|n|n>super::super::rpc_stream::MAX_STREAM_BYTES){self.close();return Err(CodecError::TooLarge);}
        self.received_replies.push(reply);Ok(())
    }
}
