//! Original OfflineBackend-held GROUP event response state. No detached worker.
use super::*;
use super::super::{group_event_delivery::{RelationEventBinding,GroupEventTicket},group_event_inbox::CommittedGroupEvent};
#[derive(Default)]
pub(super) struct GroupEventState{pub binding:Option<std::sync::Arc<RelationEventBinding>>,pub pending:BTreeMap<(u32,i32),([u8;32],std::time::Instant)>,pub completed:BTreeMap<(u32,i32),(GroupEventTicket,CommittedGroupEvent)>}
impl GroupEventState{pub(super) fn clear(&mut self){self.binding=None;self.pending.clear();self.completed.clear();}}
impl OfflineBackend{
    pub(in super::super) fn arm_relation_events(&mut self,config:super::super::group_events::NativeRelationEventsConfig)->Result<(),ProtocolError>{self.protocol.arm_relation_events(config)}
    pub(in super::super) fn complete_group_event(&mut self,ticket:&GroupEventTicket,proof:Option<&CommittedGroupEvent>)->Result<Option<CallCompletion>,ProtocolError>{
        let scope=self.connection_scope();if !ticket.matches(ticket.notice().binding().im().owner(),&scope)||proof.is_some_and(|p|!p.authorizes(ticket,std::time::Instant::now())){return Ok(None);}
        match ticket.source(){
            super::super::im_delivery::ImReplySource::Direct(_)=>self.protocol.complete_group_event(ticket,proof).map(|p|p.map(CallCompletion::Direct)),
            super::super::im_delivery::ImReplySource::Batch(inner)=>{
                let key=(inner.peer_incarnation(),inner.sequence());
                if self.group_events.pending.get(&key)!=Some(&(*ticket.fingerprint(),ticket.deadline())){return Ok(None);}
                if proof.is_some()&&self.im_completed.len()+self.status_completed.len()+self.group_events.completed.len()>=super::super::rpc_stream::MAX_MAP_ENTRIES{return Err(CodecError::TooLarge.into());}
                let result=super::super::group_event_packets::result(proof.is_some())?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(inner,0,result.expose_to_transport())?{
                    self.group_events.pending.remove(&key);if let Some(proof)=proof{self.group_events.completed.insert(key,(ticket.captured_copy(),proof.captured_copy()));}
                    Ok(Some(CallCompletion::BatchQueued))
                }else{Ok(None)}
            }
        }
    }
}
