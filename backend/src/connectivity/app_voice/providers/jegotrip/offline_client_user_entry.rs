//! Original Client-bound UserEntry work shares hosting's nested sequence and
//! scheduler. It is not a raw/direct Common-envelope submission API.
use super::*;
use super::super::{user_entry_client::{ClientUserEntryQuery,ClientUserEntryCompletion},user_entry_rpc::UserEntryError};
use std::sync::Arc;
impl OfflineBackend{
    pub(in super::super) fn client_read_state(&self)->&super::super::transport_actor::ClientReadState{&self.client_actor}
    pub(in super::super) fn client_read_state_mut(&mut self)->&mut super::super::transport_actor::ClientReadState{&mut self.client_actor}
    pub(in super::super) fn queue_client_user_entry(&mut self,query:Arc<ClientUserEntryQuery>)->Result<i32,UserEntryError>{
        if self.phase()!=ArcPhase::Authenticated||self.scheduler.is_none()||!query.matches(&self.tracker.batch_owner().map_err(|_|UserEntryError::Scope)?,&self.connection_scope()){return Err(UserEntryError::Scope);}
        let(auth,budget)=self.protocol.capture_client_user_entry_auth(&query)?;
        let request=self.tracker.prepare_client_user_entry(query,auth,budget)?;let sequence=request.sequence();
        if let Err(error)=self.scheduler.as_mut().ok_or(UserEntryError::Scope)?.queue_client_user_entry(request){
            self.tracker.retire_client_user_entry(sequence);return Err(match error{super::super::batch_scheduler::BatchError::Backpressure|super::super::batch_scheduler::BatchError::Codec(CodecError::TooLarge)=>UserEntryError::Capacity,_=>UserEntryError::Scope});
        }
        Ok(sequence)
    }
    pub(in super::super) fn retire_client_user_entry(&mut self,sequence:i32){
        self.tracker.retire_client_user_entry(sequence);
        if let Some(scheduler)=&mut self.scheduler{scheduler.retire_client_read(sequence);}
    }
    pub(in super::super) fn take_client_user_entry_replies(&mut self)->Vec<ClientUserEntryCompletion>{
        let replies=self.tracker.take_client_user_entry_replies();
        if let Some(scheduler)=&mut self.scheduler{for reply in &replies{scheduler.retire_client_read(reply.request.sequence());}}
        replies
    }
}
