//! Start/Join the independent Alive resource only on this original main attempt.
use super::*;
use super::super::{alive_runtime::{AliveBudgets,AliveRun},runtime_provision::AccountAliveState};
impl Attempt{
    pub(super) async fn start_alive(&self,profile:&RuntimeProfile)->Result<(),BackendFailure>{
        let Some(policy)=&profile.account_alive else{return Ok(());};
        let Some(link)=&self.alive_link else{return Err(BackendFailure::Unavailable);};
        let Some(receiver)=&profile.native_receivers else{return Err(BackendFailure::Unavailable);};
        let mut slot=self.alive_handle.lock().await;
        let reference={let state=lock(&self.state);
            if state.retired||state.unregister_started||state.logout_started||state.alive_stopping||!self.scope.is_open(){return Ok(());}
            if slot.is_some(){return Err(BackendFailure::Unavailable);}
            match state.account_alive{Some(AccountAliveState::Absent|AccountAliveState::Disabled)=>return Ok(()),_=>{}}
            state.alive_reference.clone().ok_or(BackendFailure::Unavailable)?};
        let decoded=match super::super::legacy_config::decode_configuration(&reference,profile.reference_decode_key.as_ref()){
            Ok(value)=>value,Err(_)=>{link.idle("unapproved");return Ok(());}};
        let common=profile.common.as_ref().ok_or(BackendFailure::Unavailable)?;
        let run=AliveRun::start(link.clone(),&self.owner,&self.scope,decoded.expose_to_backend(),policy,receiver.clone(),AliveBudgets{
            connect:common.connect_timeout,rpc:profile.rpc_timeout,partial:profile.partial_timeout,io:profile.io_timeout}).map_err(|_|BackendFailure::Unavailable)?;
        *slot=Some(run);Ok(())
    }
    pub(super) async fn stop_alive(&self)->Result<(),BackendFailure>{
        lock(&self.state).alive_stopping=true;
        // Retain the original handle through cancellation; a later cleanup may
        // finish this Join, but can never submit a replacement registration.
        let mut slot=self.alive_handle.lock().await;
        if let Some(run)=slot.as_mut(){run.close().await.map_err(|_|BackendFailure::Disconnected)?;}
        *slot=None;if let Some(link)=&self.alive_link{link.close();}Ok(())
    }
}
