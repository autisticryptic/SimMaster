//! Matched original bootstrap data -> separately pre-bound account TLS scope.
//! No locator/default/host/root inference, SDK callback or active-IO mutation.
//! Captured fields are DATA after source closure; destination authorization
//! still requires the original scope/owner/explicit imported trust identity.
use super::{arc_session::OwnedBootstrapReply,deployment::BootstrapDeployment,
    legacy_config::ConfigDecodeKey,rpc_pending::RpcConnectionScope,rpc_stream::MAX_FIELD_BYTES,
    runtime_connector::{validate_explicit_profile,validate_tls_anchor,ConnectionInput},session::AttemptOwner};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ProvisionError {Scope,Configuration,UnsupportedReference,MissingProperty,InvalidProperty,ResourceLimit}
impl ProvisionError {
    pub fn code(self)->&'static str{match self {
        Self::Scope=>"jegotrip_provision_scope_invalid",Self::Configuration=>"jegotrip_provision_configuration_invalid",
        Self::UnsupportedReference=>"jegotrip_provision_reference_unsupported",Self::MissingProperty=>"jegotrip_provision_property_missing",
        Self::InvalidProperty=>"jegotrip_provision_property_invalid",Self::ResourceLimit=>"jegotrip_provision_resource_limit",
    }}
}
pub struct AccountTrustInputs<'a>{pub authority:&'a str,pub root_der_base64:&'a str,pub enable_sni:bool}
impl fmt::Debug for AccountTrustInputs<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AccountTrustInputs(<redacted>)")}}
pub struct AccountTrust{authority:Zeroizing<String>,root:Zeroizing<String>,sni:bool,identity:Arc<()>}
impl fmt::Debug for AccountTrust{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AccountTrust(<redacted>)")}}
impl AccountTrust{
    pub fn supplied(input:AccountTrustInputs<'_>)->Result<Self,ProvisionError>{
        validate_tls_anchor(input.authority,input.root_der_base64).map_err(|_|ProvisionError::Configuration)?;
        Ok(Self{authority:Zeroizing::new(input.authority.to_owned()),root:Zeroizing::new(input.root_der_base64.to_owned()),sni:input.enable_sni,identity:Arc::new(())})
    }
    pub(super) fn authority(&self)->&str{&self.authority}
    pub(super) fn root_der_base64(&self)->&str{&self.root}
    pub(super) fn enable_sni(&self)->bool{self.sni}
}

pub struct ProvisionedLogin {
    owner:AttemptOwner,source:RpcConnectionScope,destination:RpcConnectionScope,trust:Arc<()>,call_id:i32,
    reference:Zeroizing<String>,domain_name:Zeroizing<Vec<u8>>,domain_id:Zeroizing<Vec<u8>>,app_id:Zeroizing<Vec<u8>>,cc:Zeroizing<Vec<u8>>,
}
impl fmt::Debug for ProvisionedLogin{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ProvisionedLogin(<redacted>)")}}
pub(super) struct ProvisionedFields<'a>{pub domain_name:&'a [u8],pub domain_id:&'a [u8],pub application_id:&'a [u8],pub device_info_cc:&'a [u8]}
impl fmt::Debug for ProvisionedFields<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ProvisionedFields(<redacted>)")}}
fn field(value:&[u8],empty:bool,total:&mut usize)->Result<Zeroizing<Vec<u8>>,ProvisionError>{
    let text=std::str::from_utf8(value).map_err(|_|ProvisionError::InvalidProperty)?;
    if (!empty&&text.trim().is_empty())||text.chars().any(char::is_control)||text.contains("ENC("){return Err(ProvisionError::InvalidProperty);}
    if value.len()>MAX_FIELD_BYTES{return Err(ProvisionError::ResourceLimit);}
    *total=total.checked_add(value.len()).ok_or(ProvisionError::ResourceLimit)?;
    if *total>128*1024{return Err(ProvisionError::ResourceLimit);}Ok(Zeroizing::new(value.to_vec()))
}
impl ProvisionedLogin {
    /// No caller-provided uncorrelated result/HTTP DTO can construct this plan.
    /// Decode keys are separate CLIENT properties vs Common reference contexts.
    pub fn from_reply(reply:&OwnedBootstrapReply,owner:&AttemptOwner,source:&RpcConnectionScope,destination:&RpcConnectionScope,
        trust:&AccountTrust,reference_key:Option<&ConfigDecodeKey>,client_key:Option<&ConfigDecodeKey>)->Result<Self,ProvisionError>{
        if source.matches(destination)||!destination.is_open(){return Err(ProvisionError::Scope);}
        let material=reply.material_for(owner,source).ok_or(ProvisionError::Scope)?;
        let deployment=BootstrapDeployment::from_reply(reply,owner,source,reference_key).map_err(|_|ProvisionError::Configuration)?;
        let reference=deployment.account_reference_for(owner,source).ok_or(ProvisionError::Scope)?;
        reference.direct_raw_tcp().map_err(|_|ProvisionError::UnsupportedReference)?;
        let encoded=reference.encode_native().map_err(|_|ProvisionError::UnsupportedReference)?;
        let text=std::str::from_utf8(encoded.expose_to_backend()).map_err(|_|ProvisionError::UnsupportedReference)?;
        validate_explicit_profile(ConnectionInput{server_reference:text,authority:trust.authority(),root_der_base64:trust.root_der_base64(),
            enable_sni:trust.enable_sni(),reference_decode_key:None}).map_err(|_|ProvisionError::UnsupportedReference)?;
        let raw_domain=material.properties.iter().find(|p|p.key==b"DomainName").ok_or(ProvisionError::MissingProperty)?.value;
        let mut total=text.len();let domain_name=field(raw_domain,false,&mut total)?;
        if domain_name.iter().any(|b|matches!(b,b'@'|b'['|b']')){return Err(ProvisionError::InvalidProperty);}
        let decoded=|name:&[u8]|deployment.property_for(owner,source,name,client_key).map_err(|_|ProvisionError::Configuration);
        let domain=decoded(b"DomainId")?.ok_or(ProvisionError::MissingProperty)?;
        let domain_id=field(domain.expose_to_backend(),false,&mut total)?;
        let app=decoded(b"AppId")?.ok_or(ProvisionError::MissingProperty)?;
        let app_id=field(app.expose_to_backend(),false,&mut total)?;
        let cc=decoded(b"CC")?;let cc=field(cc.as_ref().map_or(b"".as_slice(),|value|value.expose_to_backend()),true,&mut total)?;
        Ok(Self{owner:reply.owner().clone(),source:source.clone(),destination:destination.clone(),trust:trust.identity.clone(),call_id:reply.call_id(),
            reference:Zeroizing::new(text.to_owned()),domain_name,domain_id,app_id,cc})
    }
    fn matches(&self,owner:&AttemptOwner,destination:&RpcConnectionScope)->bool{
        &self.owner==owner&&self.destination.matches(destination)&&destination.is_open()
    }
    pub(super) fn connection_for<'a>(&'a self,owner:&AttemptOwner,destination:&RpcConnectionScope,trust:&'a AccountTrust)->Result<ConnectionInput<'a>,ProvisionError>{
        if !self.matches(owner,destination)||!Arc::ptr_eq(&self.trust,&trust.identity){return Err(ProvisionError::Scope);}
        Ok(ConnectionInput{server_reference:&self.reference,authority:trust.authority(),root_der_base64:trust.root_der_base64(),enable_sni:trust.enable_sni(),reference_decode_key:None})
    }
    pub(super) fn fields_for(&self,owner:&AttemptOwner,destination:&RpcConnectionScope)->Result<ProvisionedFields<'_>,ProvisionError>{
        if !self.matches(owner,destination){return Err(ProvisionError::Scope);}
        Ok(ProvisionedFields{domain_name:&self.domain_name,domain_id:&self.domain_id,application_id:&self.app_id,device_info_cc:&self.cc})
    }
    pub(super) fn original_source_scope(&self)->RpcConnectionScope{self.source.clone()}
    pub fn bootstrap_call_id(&self)->i32{self.call_id}
}

#[cfg(test)]
#[path="runtime_provision_tests.rs"]
mod tests;
