//! Actual original reply-packet lifetime, independent of Client READ and
//! outbound relation WRITE holders. Mixed batches retain BOTH typed owners.
use super::{group_event_delivery::GroupEventTicket,group_event_inbox::CommittedGroupEvent,native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,Writer,RpcBytes,MAX_MAP_ENTRIES}};
use std::{fmt,sync::Arc,time::Instant};
pub(super) struct GroupEventPackets{entries:Vec<(GroupEventTicket,Option<CommittedGroupEvent>)>,org:Vec<(super::im_delivery::ImReplyTicket,super::im_inbox::CommittedIm)>,_budget:Arc<NativeRpcReservation>,_org_budget:Option<Arc<NativeRpcReservation>>,buddy_check:Option<Arc<super::rpc_im::ImRequest>>,management:Option<Arc<super::rpc_relation_update::RelationRpcRequest>>,created_read:Option<Arc<super::rpc_im::ImRequest>>}
impl fmt::Debug for GroupEventPackets{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("GroupEventPackets(<redacted>)")}}
impl GroupEventPackets{
    /// Charge header copies BEFORE allocating the retained ticket list. The
    /// immutable body/binding reservations remain held by each original notice.
    pub(super) fn capture<'a>(scope:&RpcConnectionScope,entries:impl ExactSizeIterator<Item=(&'a GroupEventTicket,Option<&'a CommittedGroupEvent>)>)->Result<Arc<Self>,CodecError>{
        let count=entries.len();if count==0||count>MAX_MAP_ENTRIES{return Err(CodecError::InvalidField);}
        let bytes=count.checked_mul(std::mem::size_of::<(GroupEventTicket,Option<CommittedGroupEvent>)>()).and_then(|n|n.checked_add(std::mem::size_of::<Self>()+128)).ok_or(CodecError::TooLarge)?;
        let budget=scope.reserve_native_rpc(bytes)?;let mut captured=Vec::with_capacity(count);
        for(ticket,proof)in entries{
            if !ticket.matches(ticket.notice().binding().im().owner(),scope)||proof.is_some_and(|p|!p.authorizes(ticket,Instant::now())){return Err(CodecError::InvalidField);}
            captured.push((ticket.captured_copy(),proof.map(CommittedGroupEvent::captured_copy)));
        }
        Ok(Arc::new(Self{entries:captured,org:Vec::new(),_budget:budget,_org_budget:None,buddy_check:None,management:None,created_read:None}))
    }
    pub(super) fn capture_with_org<'a>(scope:&RpcConnectionScope,group:impl ExactSizeIterator<Item=(&'a GroupEventTicket,Option<&'a CommittedGroupEvent>)>,im:impl Iterator<Item=(&'a super::im_delivery::ImReplyTicket,&'a super::im_inbox::CommittedIm)>+Clone)->Result<Option<Arc<Self>>,CodecError>{
        let count=im.clone().filter(|(_,p)|p.org_receipt()).count();if group.len()==0&&count==0{return Ok(None);}if group.len()+count>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
        let mut value=if group.len()>0{Self::capture(scope,group)?}else{Arc::new(Self{entries:Vec::new(),org:Vec::new(),_budget:scope.reserve_native_rpc(std::mem::size_of::<Self>()+128)?,_org_budget:None,buddy_check:None,management:None,created_read:None})};
        if count>0{let bytes=count.checked_mul(std::mem::size_of::<(super::im_delivery::ImReplyTicket,super::im_inbox::CommittedIm)>()).and_then(|n|n.checked_add(128)).ok_or(CodecError::TooLarge)?;let budget=scope.reserve_native_rpc(bytes)?;
            let own=Arc::get_mut(&mut value).ok_or(CodecError::InvalidField)?;own.org=Vec::with_capacity(count);own._org_budget=Some(budget);
            for(t,p)in im.filter(|(_,p)|p.org_receipt()){if !t.matches(t.binding().owner(),scope)||!p.authorizes(t,Instant::now()){return Err(CodecError::InvalidField);}own.org.push((t.captured_copy(),p.captured_copy()));}
        }Ok(Some(value))
    }
    /// Shared actual-packet guard lane; Buddy queries carry no event proof and
    /// cannot complete any incoming receipt. This reuses only physical lifetime.
    pub(super) fn capture_buddy_check(scope:&RpcConnectionScope,request:Arc<super::rpc_im::ImRequest>)->Result<Arc<Self>,CodecError>{
        if !request.query().buddy_check_query()||!request.matches(request.binding().owner(),scope){return Err(CodecError::InvalidField);}
        let budget=scope.reserve_native_rpc(std::mem::size_of::<Self>()+128)?;Ok(Arc::new(Self{entries:Vec::new(),org:Vec::new(),_budget:budget,_org_budget:None,buddy_check:Some(request),management:None,created_read:None}))
    }
    pub(super) fn capture_management(scope:&RpcConnectionScope,request:Arc<super::rpc_relation_update::RelationRpcRequest>)->Result<Arc<Self>,CodecError>{
        if request.permit().management_operation().is_none()||!request.matches(request.permit().binding().owner(),scope){return Err(CodecError::InvalidField);}let budget=scope.reserve_native_rpc(std::mem::size_of::<Self>()+128)?;
        Ok(Arc::new(Self{entries:Vec::new(),org:Vec::new(),_budget:budget,_org_budget:None,buddy_check:None,management:Some(request),created_read:None}))
    }
    pub(super) fn capture_created_read(scope:&RpcConnectionScope,request:Arc<super::rpc_im::ImRequest>)->Result<Arc<Self>,CodecError>{
        if !request.query().created_properties_query()||!request.matches(request.binding().owner(),scope){return Err(CodecError::InvalidField);}let budget=scope.reserve_native_rpc(std::mem::size_of::<Self>()+128)?;
        Ok(Arc::new(Self{entries:Vec::new(),org:Vec::new(),_budget:budget,_org_budget:None,buddy_check:None,management:None,created_read:Some(request)}))
    }
    pub(super) fn current(&self,now:Instant)->bool{self.created_read.as_ref().is_none_or(|r|r.active())&&self.management.as_ref().is_none_or(|r|r.permit().packet_current(r.id()))&&self.buddy_check.as_ref().is_none_or(|r|r.active())&&self.org.iter().all(|(t,p)|p.authorizes(t,now))&&self.entries.iter().all(|(ticket,proof)|{
        let im=ticket.notice().binding().im();ticket.matches(im.owner(),&im.scope())&&proof.as_ref().is_none_or(|p|p.authorizes(ticket,now))
    })}
}
/// Byte-identical original six-byte type0/BOOL/empty-footer result, with exact
/// measured allocation rather than a new full-stream scratch allocation.
pub(super) fn result(accepted:bool)->Result<RpcBytes,CodecError>{let mut w=Writer::measured(6)?;w.boolean(accepted)?;w.bytes(b"")?;Ok(w.finish())}
/// The receipt/database check runs OUTSIDE poll_before's nonblocking original
/// IO gate. Its same pinned future still performs the unchanged scope/deadline
/// check immediately before each physical write/flush poll.
pub(super) async fn poll_original_write<F,T>(original:F,event:Option<&Arc<GroupEventPackets>>)->Result<T,super::runtime_connector::ConnectError>
where F:std::future::Future<Output=Result<T,super::runtime_connector::ConnectError>>{
    tokio::pin!(original);
    std::future::poll_fn(|cx|{
        if event.is_some_and(|e|!e.current(Instant::now())){return std::task::Poll::Ready(Err(super::runtime_connector::ConnectError::Scope));}
        std::future::Future::poll(original.as_mut(),cx)
    }).await
}
pub(super) fn direct_reply(id:i32,accepted:bool)->Result<RpcBytes,CodecError>{
    if id<=0{return Err(CodecError::InvalidCallId);}let result=result(accepted)?;let mut w=Writer::measured(19)?;w.i32(id)?;w.u32(0)?;w.bytes(result.expose_to_transport())?;Ok(w.finish())
}
