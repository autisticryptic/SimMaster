//! Independent AccountAlive configuration. Legacy stcp is explicitly NOT TLS.
//! Only a matching original bootstrap reference may select a configured peer.
use super::{object_reference::{parse_object_reference,ReferenceTarget},rpc_stream::MAX_FIELD_BYTES};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
use zeroize::Zeroize;
#[derive(Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountAliveConfig{
    pub allow_legacy_stcp:bool,
    #[serde(default="default_interval")]
    pub interval_seconds:u32,
    pub endpoints:Vec<AccountAliveEndpoint>,
}
fn default_interval()->u32{180}
#[derive(Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountAliveEndpoint{pub host:String,pub port:u16}
impl fmt::Debug for AccountAliveConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AccountAliveConfig(<redacted>)")}}
impl fmt::Debug for AccountAliveEndpoint{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AccountAliveEndpoint(<redacted>)")}}
impl Drop for AccountAliveEndpoint{fn drop(&mut self){self.host.zeroize();}}
#[derive(Clone)]
pub(super) struct AlivePolicy{pub allow_legacy_stcp:bool,pub interval:u32,endpoints:Arc<Vec<AccountAliveEndpoint>>}
impl fmt::Debug for AlivePolicy{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AlivePolicy(<redacted>)")}}
impl AlivePolicy{
    pub(super) fn capture(config:&AccountAliveConfig)->Result<Self,&'static str>{
        if config.interval_seconds!=0&&!(60..=600).contains(&config.interval_seconds){return Err("app_account_alive_configuration_invalid");}
        if config.endpoints.len()>16||config.interval_seconds!=0&&config.endpoints.is_empty(){return Err("app_account_alive_configuration_invalid");}
        for (index,endpoint) in config.endpoints.iter().enumerate(){
            if endpoint.port==0||!super::tls_engine::valid_authority(&endpoint.host)||endpoint.host.len()>253
                ||config.endpoints[..index].iter().any(|old|old.host==endpoint.host&&old.port==endpoint.port){return Err("app_account_alive_configuration_invalid");}
        }
        Ok(Self{allow_legacy_stcp:config.allow_legacy_stcp,interval:config.interval_seconds,endpoints:Arc::new(config.endpoints.clone())})
    }
    pub(super) fn bytes(&self)->usize{self.endpoints.iter().map(|e|e.host.len()+2).sum()}
    pub(super) fn select(&self,reference:&[u8])->Result<AccountAliveEndpoint,&'static str>{
        if !self.allow_legacy_stcp{return Err("app_account_alive_transport_not_approved");}
        if reference.len()>MAX_FIELD_BYTES{return Err("app_account_alive_reference_invalid");}
        let reference=parse_object_reference(reference).map_err(|_|"app_account_alive_reference_invalid")?;
        if reference.identity()!=b"AccountAlive"||reference.components().len()!=1{return Err("app_account_alive_reference_invalid");}
        let ReferenceTarget::Direct(endpoints)=reference.target()else{return Err("app_account_alive_reference_invalid");};
        // No TLS scheme downgrade, arbitrary selection or hidden retry across
        // returned targets. Multi-endpoint routing requires a separate policy.
        if endpoints.len()!=1||endpoints[0].scheme()!=b"stcp"||endpoints[0].router_id().is_some(){return Err("app_account_alive_reference_invalid");}
        self.endpoints.iter().find(|e|e.host.as_bytes()==endpoints[0].host()&&e.port==endpoints[0].port()).cloned().ok_or("app_account_alive_endpoint_unapproved")
    }
}
