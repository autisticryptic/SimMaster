//! One explicit communication operation on the admitted original App attempt.
//! Store uncertainty BEFORE a write; future cancellation/restart never retries it.
use super::*;
use super::communication::{CommunicationAction,CommunicationObservation,CommunicationState};
impl AppLoginManager{
    pub(super) async fn run_communication(&self,owner:&str,id:&str,revision:u64,client:&mut AppClient,guard:&mut ActiveGuard,bound:&http::BoundNumber)->Result<(),Error>{
        let operation=client.communication_operation().cloned().ok_or(Error::new("app_login_request_invalid"))?;
        if self.selected_configuration().map_err(Error::new)?.1!=revision{return Err(Error::new("app_login_configuration_revision_conflict"));}
        let key=client.account_key(bound)?;self.confirm_device_identity(guard,&key,true)?;
        if operation.action().writes()&&self.database.with_conn(|c|device_identity::native_ownership_pending(c,&key)).map_err(Error::new)?{return Err(Error::new("app_login_device_identity_in_use"));}
        self.step(owner,id,"communication_request",None)?;
        let mut observation=CommunicationObservation::initial(&operation,revision,&guard.snapshot.id,guard.snapshot.revision,now());
        observation.state=CommunicationState::RequestUncertain;
        self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;
        let outcome=client.perform_communication(bound).await;
        match outcome{
            Err(error)=>{
                observation.error=Some(error.code.to_owned());observation.server_code=error.server_code;
                if error.server_code.is_some(){observation.state=CommunicationState::Rejected;}
                observation.observed_at=now();self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;return Err(error);
            }
            Ok(status)=>{
                observation.status=status;observation.state=if operation.action().writes(){CommunicationState::AcceptedUnconfirmed}else{CommunicationState::Observed};
            }
        }
        observation.observed_at=now();self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;
        if !operation.action().writes(){return Ok(());}
        if operation.action()==CommunicationAction::CloseForward{
            // The original login callback emits CloseVoipForward only on
            // code0/body.cs_forward=2. This is an API acknowledgement, NOT
            // getCallStatus=1 or a claim that hosting has been disabled.
            if observation.status.as_ref().is_some_and(|status|status.cs_forward==Some(2)){
                observation.state=CommunicationState::Acknowledged;
                self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;return Ok(());
            }
            return Err(Error::new("app_login_communication_unconfirmed"));
        }
        if operation.action()==CommunicationAction::Logout{
            // This clears only the App client's session. No native logout, line
            // deletion or remote forwarding state is inferred from that result.
            observation.state=CommunicationState::Confirmed;self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;return Ok(());
        }
        self.step(owner,id,"communication_confirm",None)?;
        if self.selected_configuration().map_err(Error::new)?.1!=revision{return Err(Error::new("app_login_configuration_revision_conflict"));}
        let confirmation=client.confirm_communication().await;
        match confirmation{
            Ok(Some(status))=>{
                let confirmed=client.communication_matches(&status);observation.status=Some(status);
                if confirmed{observation.state=CommunicationState::Confirmed;}else{observation.error=Some("app_login_communication_unconfirmed".to_owned());}
            }
            Ok(None)=>observation.error=Some("app_login_communication_unconfirmed".to_owned()),
            Err(error)=>{observation.error=Some(error.code.to_owned());observation.server_code=error.server_code;},
        }
        observation.observed_at=now();self.database.with_conn(|c|store::set_communication(c,id,owner,&observation,now())).map_err(Error::new)?;
        if observation.state==CommunicationState::Confirmed{Ok(())}else{Err(Error::new("app_login_communication_unconfirmed"))}
    }
}
