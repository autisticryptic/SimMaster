//! Same original Common SDK actor; incoming relation observation is not a
//! relationship WRITE result, readiness callback or permission to recover IO.
use super::*;
impl NativeHostingBackend{
    pub(in super::super) fn relation_events_enabled(&self)->bool{self.profile.relation_events.is_some()&&self.im_messages_enabled()}
    pub(in super::super) async fn complete_group_event(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,ticket:&super::super::group_event_delivery::GroupEventTicket,proof:Option<&super::super::group_event_inbox::CommittedGroupEvent>)->Result<RuntimeReceipt,RuntimeError>{
        if !self.relation_events_enabled(){return Err(RuntimeError::InvalidConfiguration);}
        let attempt=self.original(owner).map_err(|_|RuntimeError::Scope)?;attempt.commands(owner,scope)?.complete_group_event(owner,scope,ticket,proof).await
    }
}
