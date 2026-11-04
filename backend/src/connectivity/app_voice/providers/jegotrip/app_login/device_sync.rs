//! Original Android device/v1/sync report, explicitly supplied phone context.
//! This is a device report, not a VTD grant, hardware emulator or push token mint.
use super::{config::valid_app_version, crypto::JsonDoc, stored_code, Error};
use super::super::credentials::Secret;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceSyncProfile {
    /// Keep the observed App version/code pair; never derive a code from dots.
    pub app_version: Secret,
    pub app_version_code: u32,
    pub device_type: Secret,
    pub os_version: Secret,
    pub channel_id: Secret,
    #[serde(default)] pub device_token: Option<Secret>,
    #[serde(default)] pub mac_address: Option<Secret>,
    #[serde(default)] pub terminal_brand: Option<Secret>,
    #[serde(default)] pub terminal_model: Option<Secret>,
    #[serde(default)] pub resolution: Option<Secret>,
}
impl fmt::Debug for DeviceSyncProfile {
    fn fmt(&self, f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("DeviceSyncProfile(<redacted>)") }
}
impl DeviceSyncProfile {
    pub(super) fn validate(&self)->Result<(),Error> {
        let invalid=||Error::new("app_login_deployment_invalid");
        if !valid_app_version(self.app_version.expose_to_backend()) || self.app_version_code==0 || self.app_version_code>i32::MAX as u32 {
            return Err(invalid());
        }
        for field in [&self.device_type,&self.os_version,&self.channel_id] {
            if !valid_text(field.expose_to_backend(),128) { return Err(invalid()); }
        }
        for field in [&self.mac_address,&self.terminal_brand,&self.terminal_model,&self.resolution].into_iter().flatten() {
            if !valid_text(field.expose_to_backend(),128) { return Err(invalid()); }
        }
        // Missing push context is OMITTED, not guessed or cleared by "".
        if self.device_token.as_ref().is_some_and(|v|!valid_text(v.expose_to_backend(),4096)) { return Err(invalid()); }
        Ok(())
    }
    pub(super) fn payload(&self, device_id:&str, account_id:&str)->Result<JsonDoc,Error> {
        self.validate()?;
        if !super::device_identity::valid_phone_app_uuid(device_id) || !valid_text(account_id,254) {
            return Err(Error::new("app_login_device_sync_context_missing"));
        }
        let mut value=serde_json::json!({
            "app_version":self.app_version.expose_to_backend(),"app_version_code":self.app_version_code,
            "device_id":device_id,"imei":device_id,"device_type":self.device_type.expose_to_backend(),
            "os_type":"0","os_version":self.os_version.expose_to_backend(),"channel_id":self.channel_id.expose_to_backend(),
            "userId":account_id,
        });
        let map=value.as_object_mut().ok_or(Error::new("app_login_request_invalid"))?;
        for (key,field) in [("device_token",&self.device_token),("macAddress",&self.mac_address),
            ("terminalBrand",&self.terminal_brand),("terminalModel",&self.terminal_model),("resolution",&self.resolution)] {
            if let Some(field)=field { map.insert(key.to_owned(),serde_json::Value::String(field.expose_to_backend().to_owned())); }
        }
        Ok(JsonDoc(value))
    }
    pub(super) fn private_mirror(&self)->PrivateDeviceSyncProfileRef<'_> {
        PrivateDeviceSyncProfileRef {app_version:self.app_version.expose_to_backend(),app_version_code:self.app_version_code,
            device_type:self.device_type.expose_to_backend(),os_version:self.os_version.expose_to_backend(),channel_id:self.channel_id.expose_to_backend(),
            device_token:self.device_token.as_ref().map(Secret::expose_to_backend),mac_address:self.mac_address.as_ref().map(Secret::expose_to_backend),
            terminal_brand:self.terminal_brand.as_ref().map(Secret::expose_to_backend),terminal_model:self.terminal_model.as_ref().map(Secret::expose_to_backend),
            resolution:self.resolution.as_ref().map(Secret::expose_to_backend)}
    }
}
fn valid_text(value:&str,max:usize)->bool { !value.is_empty() && value.len()<=max && !value.chars().any(char::is_control) }
#[derive(Serialize)]
pub(super) struct PrivateDeviceSyncProfileRef<'a> {
    app_version:&'a str, app_version_code:u32, device_type:&'a str, os_version:&'a str, channel_id:&'a str,
    device_token:Option<&'a str>,mac_address:Option<&'a str>,terminal_brand:Option<&'a str>,terminal_model:Option<&'a str>,resolution:Option<&'a str>,
}

#[derive(Clone,Copy,Debug,Deserialize,Serialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum DeviceSyncState { NotConfigured, NotRequested, RequestUncertain, Accepted, Rejected }
/// Bounded historical outcome only; neither report contents nor a live grant.
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSyncObservation {
    pub state:DeviceSyncState,
    pub error:Option<String>,
    pub server_code:Option<u32>,
}
impl DeviceSyncObservation {
    pub(super) fn not_configured()->Self {Self {state:DeviceSyncState::NotConfigured,error:None,server_code:None}}
    pub(super) fn not_requested()->Self {Self {state:DeviceSyncState::NotRequested,error:None,server_code:None}}
    pub(super) fn request_uncertain()->Self {Self {state:DeviceSyncState::RequestUncertain,error:None,server_code:None}}
    pub(super) fn capture(result:Result<(),Error>)->Self {
        match result {
            Ok(())=>Self {state:DeviceSyncState::Accepted,error:None,server_code:None},
            Err(error)=>Self {state:if error.server_code.is_some() {DeviceSyncState::Rejected}else{DeviceSyncState::RequestUncertain},
                error:Some(stored_code(error.code).unwrap_or("app_login_internal_failed").to_owned()),server_code:error.server_code},
        }
    }
    pub(super) fn valid(&self)->bool {
        let valid_error=self.error.as_deref().is_some_and(|value|stored_code(value).is_some());
        match self.state {
            DeviceSyncState::NotConfigured|DeviceSyncState::NotRequested|DeviceSyncState::Accepted=>self.error.is_none() && self.server_code.is_none(),
            DeviceSyncState::RequestUncertain=>self.server_code.is_none() && (self.error.is_none() || valid_error),
            DeviceSyncState::Rejected=>self.server_code.is_some() && valid_error,
        }
    }
}
