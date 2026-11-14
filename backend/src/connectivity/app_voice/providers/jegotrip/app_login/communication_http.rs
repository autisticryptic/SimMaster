//! Exact 8.9.4 communication body/protection selection on the original AppClient.
//! Never caller URLs, enrolment, SDK credentials, device reports or paid calls.
use super::*;
use super::super::communication::{CommunicationAction as Action,CommunicationOperation as Operation,CommunicationStatus};
impl AppClient{
    pub(super) fn with_communication(self,operation:Operation)->Result<Self,Error>{
        if self.local_receiver||self.trusted_reuse||self.session.is_some(){return Err(Error::new("app_login_request_invalid"));}
        if !self.config.extra_body.is_empty(){return Err(Error::new("app_login_deployment_invalid"));}
        self.config.communication.as_ref().ok_or(Error::new("app_login_communication_not_configured"))?.preflight(&operation)?;
        let mut client=self.with_trusted_reuse()?;client.communication=Some(operation);Ok(client)
    }
    pub(super) fn communication_operation(&self)->Option<&Operation>{self.communication.as_ref()}
    pub(super) async fn perform_communication(&mut self,bound:&BoundNumber)->Result<Option<CommunicationStatus>,Error>{
        let operation=self.communication.clone().ok_or(Error::new("app_login_request_invalid"))?;
        let endpoints=self.config.communication.as_ref().ok_or(Error::new("app_login_communication_not_configured"))?;endpoints.preflight(&operation)?;
        let path=if operation.action()==Action::QueryForwardingState{self.config.endpoints.call_status.clone()}else{endpoints.path(operation.action()).ok_or(Error::new("app_login_communication_not_configured"))?.to_owned()};
        let (payload,encrypted)=match &operation{
            Operation::QueryProfile|Operation::Logout=>(None,false),
            Operation::QueryDisplayStatus=>(None,true),
            Operation::QueryForwardingState=>(Some(serde_json::json!({"device_id":self.device_id()?})),false),
            Operation::QueryPhoneService=>(Some(serde_json::json!({"type":1})),false),
            Operation::SetPhoneService{status}=>(Some(serde_json::json!({"type":2,"phoneServerStatus":status})),false),
            Operation::SetVoipProfile{cs_forward}=>(Some(serde_json::json!({"cs_forward":cs_forward})),false),
            Operation::CloseForward=>(Some(serde_json::json!({"mobile":bound.mobile.expose_to_backend(),"countryCode":bound.country})),true),
            Operation::SetJegoMStatus{jegom_status,region_code,country_code}=>(Some(serde_json::json!({"jegom_status":jegom_status,"region_code":region_code,"country_code":country_code})),true),
        };
        let bytes=if let Some(payload)=payload{let payload=self.payload(payload,self.version_value().await.as_deref());Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?)}else{Zeroizing::new(Vec::new())};
        if operation.action().writes(){self.trusted_reuse_write_started.store(true,std::sync::atomic::Ordering::SeqCst);}
        let doc=self.post(&path,&bytes,Authentication::Session,encrypted).await?;
        if operation.action().writes(){
            if matches!(operation,Operation::CloseForward){return Ok(Some(CommunicationStatus::from_body(crypto::body(&doc)?)?));}
            if matches!(operation,Operation::Logout){self.reset_session();}
            return Ok(None);
        }
        let status=CommunicationStatus::from_body(crypto::body(&doc)?)?;
        if operation.action()==Action::QueryPhoneService&&status.phone_server_status.is_none(){return Err(Error::new("app_login_response_invalid"));}
        if operation.action()==Action::QueryProfile&&status.profile.is_none(){return Err(Error::new("app_login_response_invalid"));}
        if operation.action()==Action::QueryDisplayStatus&&status.jegom_status.is_none(){return Err(Error::new("app_login_response_invalid"));}
        if operation.action()==Action::QueryForwardingState&&status.cs_forward.is_none(){return Err(Error::new("app_login_response_invalid"));}
        Ok(Some(status))
    }
    pub(super) async fn confirm_communication(&self)->Result<Option<CommunicationStatus>,Error>{
        let operation=self.communication.as_ref().ok_or(Error::new("app_login_request_invalid"))?;
        if !operation.action().writes()||matches!(operation,Operation::Logout){return Ok(None);}
        let endpoints=self.config.communication.as_ref().ok_or(Error::new("app_login_communication_not_configured"))?;
        let (path,payload,encrypted)=match operation{
            Operation::SetPhoneService{..}=>(endpoints.path(Action::QueryPhoneService).ok_or(Error::new("app_login_communication_not_configured"))?,Some(serde_json::json!({"type":1})),false),
            Operation::SetJegoMStatus{..}=>(endpoints.path(Action::QueryDisplayStatus).ok_or(Error::new("app_login_communication_not_configured"))?,None,true),
            Operation::SetVoipProfile{..}|Operation::CloseForward=>(self.config.endpoints.call_status.as_str(),Some(serde_json::json!({"device_id":self.device_id()?})),false),
            _=>return Err(Error::new("app_login_request_invalid")),
        };
        let content=if let Some(payload)=payload{let payload=self.payload(payload,self.version_value().await.as_deref());Zeroizing::new(serde_json::to_vec(&payload.0).map_err(|_|Error::new("app_login_request_invalid"))?)}else{Zeroizing::new(Vec::new())};
        let doc=self.post(path,&content,Authentication::Session,encrypted).await?;Ok(Some(CommunicationStatus::from_body(crypto::body(&doc)?)?))
    }
    pub(super) fn communication_matches(&self,status:&CommunicationStatus)->bool{
        match self.communication.as_ref(){
            Some(Operation::SetPhoneService{status:expected})=>status.phone_server_status==Some(i64::from(*expected)),
            Some(Operation::SetVoipProfile{cs_forward})=>status.cs_forward==cs_forward.parse::<i64>().ok(),
            Some(Operation::CloseForward)=>status.cs_forward==Some(1),
            Some(Operation::SetJegoMStatus{jegom_status,region_code,..})=>jegom_status.parse::<i64>().ok().is_some_and(|value|status.jegom_status==Some(value))&& (jegom_status=="1"||status.region_code.as_deref()==Some(region_code.as_str())),
            _=>false,
        }
    }
}
