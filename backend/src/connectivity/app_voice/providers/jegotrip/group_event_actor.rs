//! Original actor commands for installed inbound relation processing only.
use super::*;
impl RuntimeCommands{
    pub(in super::super) async fn arm_relation_events(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,config:super::super::group_events::NativeRelationEventsConfig)->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope){return Err(RuntimeError::Scope);}self.submit(owner,scope,Operation::ArmRelationEvents(config)).await
    }
    pub(in super::super) async fn complete_group_event(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&super::super::group_event_delivery::GroupEventTicket,proof:Option<&super::super::group_event_inbox::CommittedGroupEvent>)->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope)||!ticket.matches(owner,scope)||proof.is_some_and(|p|!p.authorizes(ticket,Instant::now())){return Err(RuntimeError::Scope);}
        self.submit(owner,scope,Operation::CompleteGroupEvent(Box::new((ticket.captured_copy(),proof.map(|p|p.captured_copy()))))).await
    }
}
