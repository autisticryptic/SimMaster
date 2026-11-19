//! Original setStatus user mark-read request/reply, no scalar read grant or
//! native version-error resend. All credentials come from current ARC auth.
use super::*;
use super::super::{im_inbox::UserReadPermit,im_user_read_rpc::{UserReadRpcRequest,OwnedUserReadReply},im_user_read_data,im_status::StatusLabel};
impl ArcRpcSession{
    pub(in super::super) fn prepare_user_read_write(&mut self,binding:Arc<super::super::im_delivery::ImBinding>,permit:Arc<UserReadPermit>,deadline:std::time::Instant)->Result<(i32,RpcBytes),CodecError>{
        if !self.native_receiver_scope()||!binding.matches(&self.owner,&self.connection)||!permit.matches(&self.owner,&self.connection)||!Arc::ptr_eq(&binding,permit.binding())||std::time::Instant::now()>=deadline.min(permit.deadline()){return Err(CodecError::InvalidField);}
        if self.pending.len()>=MAX_MAP_ENTRIES||self.im_reply_count()>=MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;if !binding.authenticates(&auth.user_id,&auth.selected_session_id){return Err(CodecError::InvalidField);}
        let args=im_user_read_data::encode_mark_read(StatusLabel::parse(permit.label())?,&auth.user_id,permit.index(),permit.epoch_ms())?;let id=self.next_id.ok_or(CodecError::InvalidCallId)?;
        let bytes=encode_request(id,&[RpcProperty{key:b"id",value:super::super::rpc_im::GROUP_OBJECT},RpcProperty{key:b"token",value:&auth.token}],im_user_read_data::MARK_READ_METHOD.as_bytes(),args.expose_to_transport())?;
        let retained=bytes.expose_to_transport().len().checked_add(permit.label().len()).and_then(|n|n.checked_add(96)).ok_or(CodecError::TooLarge)?;
        let budget=self.connection.reserve_native_rpc(retained)?;
        permit.arm(id,deadline).map_err(|_|CodecError::InvalidField)?;self.next_id=id.checked_add(1);
        self.pending.insert(id,PendingRpc{kind:PendingKind::UserRead,selected_session_id:None,scheduler:None,call:None,im:None,received:None,user_read:Some(UserReadRpcRequest::original(permit,id,budget)?),send:None});Ok((id,bytes))
    }
    pub(in super::super) fn im_reply_count(&self)->usize{self.im_replies.len()+self.received_replies.len()+self.user_read_replies.len()+self.send_replies.len()}
    pub(in super::super) fn im_reply_bytes(&self)->usize{self.im_replies.iter().map(|r|r.retained_bytes()).chain(self.received_replies.iter().map(|r|r.retained_bytes())).chain(self.user_read_replies.iter().map(|r|r.retained_bytes())).chain(self.send_replies.iter().map(|r|r.retained_bytes())).sum()}
    pub(in super::super) fn take_user_read_replies(&mut self)->Vec<OwnedUserReadReply>{std::mem::take(&mut self.user_read_replies)}
    pub(in super::super) fn retire_user_read_write(&mut self,id:i32){if self.pending.get(&id).is_some_and(|p|matches!(p.kind,PendingKind::UserRead)){self.pending.remove(&id);}}
    pub(super) fn captured_user_read_reply(&mut self,request:Arc<UserReadRpcRequest>,owner:&AttemptOwner,scope:&RpcConnectionScope,code:u32,bytes:&[u8])->Result<(),CodecError>{
        if self.phase()!=ArcPhase::Authenticated||!request.matches(owner,scope){self.close();return Err(CodecError::InvalidField);}
        let reply=match OwnedUserReadReply::original(request,code,bytes){Ok(r)=>r,Err(e)=>{self.close();return Err(e);}};
        if self.im_reply_count()>=MAX_MAP_ENTRIES||self.im_reply_bytes().checked_add(reply.retained_bytes()).is_none_or(|n|n>super::super::rpc_stream::MAX_STREAM_BYTES){self.close();return Err(CodecError::TooLarge);}
        self.user_read_replies.push(reply);Ok(())
    }
}
