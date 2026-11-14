//! Original App communication operations, separate from local SDK readiness.
//! Input has no URL/token/device-grant field; write permission is explicit.
use super::{communication_profile::{CommunicationProfile,valid_region},Error,stored_code,config::HttpDeployment};
use serde::{Deserialize,Serialize};
use std::fmt;
#[derive(Clone,Copy,Debug,Deserialize,Serialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum CommunicationAction{QueryProfile,QueryPhoneService,QueryDisplayStatus,QueryForwardingState,SetPhoneService,SetVoipProfile,CloseForward,SetJegoMStatus,Logout}
impl CommunicationAction{
    pub(super) fn writes(self)->bool{!matches!(self,Self::QueryProfile|Self::QueryPhoneService|Self::QueryDisplayStatus|Self::QueryForwardingState)}
}
#[derive(Clone,Deserialize)]
// Empty struct variants are intentional: internally tagged unit variants can
// silently discard fields despite deny_unknown_fields.
#[serde(tag="kind",rename_all="snake_case",deny_unknown_fields)]
pub enum CommunicationOperation{
    QueryProfile{},QueryPhoneService{},QueryDisplayStatus{},QueryForwardingState{},
    SetPhoneService{status:i32},SetVoipProfile{cs_forward:String},CloseForward{},
    SetJegoMStatus{jegom_status:String,region_code:String,country_code:String},Logout{},
}
impl fmt::Debug for CommunicationOperation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommunicationOperation(<redacted>)")}}
impl CommunicationOperation{
    pub(super) fn action(&self)->CommunicationAction{match self{Self::QueryProfile{}=>CommunicationAction::QueryProfile,Self::QueryPhoneService{}=>CommunicationAction::QueryPhoneService,Self::QueryDisplayStatus{}=>CommunicationAction::QueryDisplayStatus,Self::QueryForwardingState{}=>CommunicationAction::QueryForwardingState,
        Self::SetPhoneService{..}=>CommunicationAction::SetPhoneService,Self::SetVoipProfile{..}=>CommunicationAction::SetVoipProfile,Self::CloseForward{}=>CommunicationAction::CloseForward,Self::SetJegoMStatus{..}=>CommunicationAction::SetJegoMStatus,Self::Logout{}=>CommunicationAction::Logout}}
    pub(super) fn validate(&self)->Result<(),Error>{
        let invalid=||Error::new("app_login_request_invalid");
        match self{
            Self::SetPhoneService{status} if !(0..=999999).contains(status)=>return Err(invalid()),
            Self::SetVoipProfile{cs_forward} if !matches!(cs_forward.as_str(),"1"|"2")=>return Err(invalid()),
            Self::SetJegoMStatus{jegom_status,region_code,country_code}=>{
                if !matches!(jegom_status.as_str(),"1"|"2")||!super::communication_profile::valid_region(country_code)
                    ||(!region_code.is_empty()&&!super::communication_profile::valid_region(region_code)){return Err(invalid());}
            }
            _=>{},
        }Ok(())
    }
}
#[derive(Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommunicationEndpoints{
    #[serde(default)] pub allow_business_writes:bool,
    pub profile:Option<String>,pub phone_service:Option<String>,pub display_status:Option<String>,pub set_voip_profile:Option<String>,pub close_forward:Option<String>,pub jego_m_status:Option<String>,pub logout:Option<String>,
}
impl CommunicationEndpoints{
    pub(super) fn validate(&self,http:&HttpDeployment)->Result<(),Error>{
        for path in [&self.profile,&self.phone_service,&self.display_status,&self.set_voip_profile,&self.close_forward,&self.jego_m_status,&self.logout].into_iter().flatten(){http.endpoint(path)?;}
        Ok(())
    }
    pub(super) fn path(&self,action:CommunicationAction)->Option<&str>{match action{
        CommunicationAction::QueryProfile=>self.profile.as_deref(),CommunicationAction::QueryForwardingState=>None,CommunicationAction::QueryPhoneService|CommunicationAction::SetPhoneService=>self.phone_service.as_deref(),CommunicationAction::QueryDisplayStatus=>self.display_status.as_deref(),
        CommunicationAction::SetVoipProfile=>self.set_voip_profile.as_deref(),CommunicationAction::CloseForward=>self.close_forward.as_deref(),CommunicationAction::SetJegoMStatus=>self.jego_m_status.as_deref(),CommunicationAction::Logout=>self.logout.as_deref(),
    }}
    pub(super) fn available(&self)->Vec<CommunicationAction>{
        [CommunicationAction::QueryProfile,CommunicationAction::QueryPhoneService,CommunicationAction::QueryDisplayStatus,CommunicationAction::QueryForwardingState,CommunicationAction::SetPhoneService,CommunicationAction::SetVoipProfile,CommunicationAction::CloseForward,CommunicationAction::SetJegoMStatus,CommunicationAction::Logout]
            .into_iter().filter(|action|(self.path(*action).is_some()||*action==CommunicationAction::QueryForwardingState)&&(!action.writes()||self.allow_business_writes)).collect()
    }
    pub(super) fn preflight(&self,operation:&CommunicationOperation)->Result<(),Error>{
        operation.validate()?;let action=operation.action();
        if action.writes()&&!self.allow_business_writes{return Err(Error::new("app_login_communication_writes_disabled"));}
        if self.path(action).is_none()&&action!=CommunicationAction::QueryForwardingState{return Err(Error::new("app_login_communication_not_configured"));}
        let confirm=match action{CommunicationAction::SetPhoneService=>Some(CommunicationAction::QueryPhoneService),CommunicationAction::SetJegoMStatus=>Some(CommunicationAction::QueryDisplayStatus),_=>None};
        if confirm.is_some_and(|a|self.path(a).is_none()){return Err(Error::new("app_login_communication_not_configured"));}Ok(())
    }
}
#[derive(Clone,Copy,Debug,Deserialize,Serialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum CommunicationState{NotRequested,RequestUncertain,AcceptedUnconfirmed,Acknowledged,Observed,Confirmed,Rejected}
#[derive(Clone,Debug,Default,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationStatus{
    pub phone_server_status:Option<i64>,pub cs_forward:Option<i64>,pub jegom_status:Option<i64>,
    pub profile:Option<CommunicationProfile>,pub region_code:Option<String>,pub masked_jegom_phone:Option<String>,
}
fn number(value:&serde_json::Value)->Option<i64>{
    let value=if let Some(v)=value.as_i64(){v}else{let text=value.as_str()?;if text.is_empty()||text.len()>6||!text.bytes().all(|b|b.is_ascii_digit()){return None;}
        let value=text.parse::<i64>().ok()?;if value.to_string()!=text{return None;}value};
    (0..=999999).contains(&value).then_some(value)
}
impl CommunicationStatus{
    pub(super) fn from_body(body:&serde_json::Map<String,serde_json::Value>)->Result<Self,Error>{
        let mut result=Self::default();
        for (key,target) in [("phoneServerStatus",&mut result.phone_server_status),("cs_forward",&mut result.cs_forward),("jegom_status",&mut result.jegom_status)]{
            if let Some(value)=body.get(key){*target=Some(number(value).ok_or(Error::new("app_login_response_invalid"))?);}
        }
        if let Some(value)=body.get("list"){
            let list=value.as_array().ok_or(Error::new("app_login_response_invalid"))?;
            if list.len()>100{return Err(Error::new("app_login_response_invalid"));}
            if let Some(first)=list.first(){result.profile=Some(CommunicationProfile::from_body(first.as_object().ok_or(Error::new("app_login_response_invalid"))?)?);}
        }
        if let Some(value)=body.get("region_code"){
            let text=value.as_str().ok_or(Error::new("app_login_response_invalid"))?;
            if !text.is_empty(){if !valid_region(text){return Err(Error::new("app_login_response_invalid"));}result.region_code=Some(text.to_owned());}
        }
        if let Some(value)=body.get("jegom_phone"){
            let text=value.as_str().ok_or(Error::new("app_login_response_invalid"))?;
            if !text.is_empty(){
                if text.len()>24||!text.bytes().all(|b|b.is_ascii_digit()||b==b'+'){return Err(Error::new("app_login_response_invalid"));}
                result.masked_jegom_phone=Some(format!("***{}",&text[text.len().saturating_sub(4)..]));
            }
        }Ok(result)
    }
    fn valid(&self)->bool{
        [self.phone_server_status,self.cs_forward,self.jegom_status].iter().flatten().all(|n|(0..=999999).contains(n))
            &&self.profile.as_ref().is_none_or(CommunicationProfile::valid)&&self.region_code.as_deref().is_none_or(valid_region)
            &&self.masked_jegom_phone.as_ref().is_none_or(|v|v.len()<=7&&v.starts_with("***")&&v[3..].bytes().all(|b|b.is_ascii_digit()||b==b'+'))
    }
}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationObservation{
    pub action:CommunicationAction,pub state:CommunicationState,pub configuration_revision:u64,pub device_identity_id:String,pub device_identity_revision:u64,pub observed_at:i64,
    pub status:Option<CommunicationStatus>,pub error:Option<String>,pub server_code:Option<u32>,
}
impl CommunicationObservation{
    pub(super) fn initial(operation:&CommunicationOperation,revision:u64,device_id:&str,device_revision:u64,at:i64)->Self{Self{action:operation.action(),state:CommunicationState::NotRequested,configuration_revision:revision,device_identity_id:device_id.to_owned(),device_identity_revision:device_revision,observed_at:at,status:None,error:None,server_code:None}}
    fn valid(&self)->bool{let status_ok=match self.state{
        CommunicationState::Observed=>match self.action{
            CommunicationAction::QueryPhoneService=>self.status.as_ref().is_some_and(|s|s.phone_server_status.is_some()),
            CommunicationAction::QueryProfile=>self.status.as_ref().is_some_and(|s|s.profile.is_some()),
            CommunicationAction::QueryDisplayStatus=>self.status.as_ref().is_some_and(|s|s.jegom_status.is_some()),
            CommunicationAction::QueryForwardingState=>self.status.as_ref().is_some_and(|s|s.cs_forward.is_some()),_=>false,
        },
        CommunicationState::Acknowledged=>self.action==CommunicationAction::CloseForward&&self.status.as_ref().is_some_and(|s|s.cs_forward==Some(2)),
        CommunicationState::Confirmed=>match self.action{
            CommunicationAction::Logout=>true,
            CommunicationAction::SetPhoneService=>self.status.as_ref().is_some_and(|s|s.phone_server_status.is_some()),
            CommunicationAction::SetVoipProfile=>self.status.as_ref().is_some_and(|s|matches!(s.cs_forward,Some(1|2))),
            CommunicationAction::SetJegoMStatus=>self.status.as_ref().is_some_and(|s|matches!(s.jegom_status,Some(1|2))),_=>false,
        },_=>true,
    };status_ok&&super::valid_attempt_id(&self.device_identity_id)&&self.device_identity_revision>0&&self.observed_at>=0&&self.configuration_revision<=i64::MAX as u64&&self.device_identity_revision<=i64::MAX as u64
        &&self.status.as_ref().is_none_or(CommunicationStatus::valid)&&self.error.as_ref().is_none_or(|v|stored_code(v).is_some())&&self.server_code.is_none_or(|n|n<=999999)
        &&(!matches!(self.state,CommunicationState::Observed|CommunicationState::Confirmed)||self.status.is_some()||self.action==CommunicationAction::Logout)}
    pub(super) fn encode(&self)->Result<String,&'static str>{if !self.valid(){return Err("app_login_storage_failed");}let value=serde_json::to_string(self).map_err(|_|"app_login_storage_failed")?;if value.len()>32768{return Err("app_login_storage_failed");}Ok(value)}
    pub(super) fn decode(value:&str)->Result<Self,&'static str>{if value.is_empty()||value.len()>32768{return Err("app_login_storage_failed");}let result:Self=serde_json::from_str(value).map_err(|_|"app_login_storage_failed")?;if !result.valid(){return Err("app_login_storage_failed");}Ok(result)}
}
