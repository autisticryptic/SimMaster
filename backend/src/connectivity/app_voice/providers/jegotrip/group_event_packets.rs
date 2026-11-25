//! Actual original reply-packet lifetime, independent of Client READ and
//! outbound relation WRITE holders. Mixed batches retain BOTH typed owners.
use super::{group_event_delivery::GroupEventTicket,group_event_inbox::CommittedGroupEvent,native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,Writer,RpcBytes,MAX_MAP_ENTRIES}};
use std::{fmt,sync::Arc,time::Instant};
pub(super) struct GroupEventPackets{entries:Vec<(GroupEventTicket,Option<CommittedGroupEvent>)>,_budget:Arc<NativeRpcReservation>}
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
        Ok(Arc::new(Self{entries:captured,_budget:budget}))
    }
    pub(super) fn current(&self,now:Instant)->bool{self.entries.iter().all(|(ticket,proof)|{
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
