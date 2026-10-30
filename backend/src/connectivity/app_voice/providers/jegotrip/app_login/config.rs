//! Private installation/deployment context. Never a public credentials export.
use super::{Error, crypto::PhoneMaterial};
use super::super::{
    credentials::{Secret, SdkCredentials, HostingCredentials, ImportedCredentials},
    account_config::{AccountConfig, ProfileConfig, BootstrapConfig, CallMediaConfig, ReconnectPolicy},
};
use serde::Deserialize;
use std::{fmt, io::Read, path::Path};
use zeroize::Zeroizing;
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Deployment {
    pub version: u32,
    pub http: HttpDeployment,
    pub runtime: RuntimeTemplate,
}
impl fmt::Debug for Deployment { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppLoginDeployment(<redacted>)") } }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BodyProtection { pub secret_id: Secret, pub secret_value: Secret }
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Endpoints {
    pub login: String, pub user_info: String, pub phone_token: String,
    pub voip_status: String, pub call_status: String, pub enable_hosting: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HttpDeployment {
    pub base_url: Secret,
    pub anonymous_token: Secret,
    pub signature_salt: Secret,
    pub password_salt: Secret,
    pub sip_password_key: Secret,
    pub device_id: Secret,
    pub endpoints: Endpoints,
    /// None is the original non-encrypted AppConfig branch, not a TLS downgrade.
    pub body_protection: Option<BodyProtection>,
    pub root_der_base64: Option<Secret>,
    pub user_agent: Option<Secret>,
    pub enroll_id: Option<Secret>,
}
impl fmt::Debug for HttpDeployment { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppHttpDeployment(<redacted>)") } }
impl HttpDeployment {
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = || Error::new("app_login_deployment_invalid");
        let base = self.base()?;
        if !base.path().ends_with('/') { return Err(invalid()); }
        for value in [&self.anonymous_token, &self.signature_salt, &self.password_salt, &self.sip_password_key, &self.device_id] {
            let text=value.expose_to_backend();
            if text.is_empty() || text.len()>16_384 || text.chars().any(char::is_control) { return Err(invalid()); }
        }
        if self.signature_salt.expose_to_backend().len()<8 || self.sip_password_key.expose_to_backend().len()!=24 { return Err(invalid()); }
        if let Some(p)=&self.body_protection {
            for v in [&p.secret_id,&p.secret_value] {
                let s=v.expose_to_backend(); if s.is_empty() || s.len()>4096 || s.chars().any(char::is_control) || s.contains(';') { return Err(invalid()); }
            }
        }
        for s in [&self.user_agent,&self.enroll_id].into_iter().flatten() {
            if s.expose_to_backend().len()>1024 || s.expose_to_backend().chars().any(char::is_control) { return Err(invalid()); }
        }
        for p in [&self.endpoints.login,&self.endpoints.user_info,&self.endpoints.phone_token,&self.endpoints.voip_status,&self.endpoints.call_status,&self.endpoints.enable_hosting] { self.endpoint(p)?; }
        Ok(())
    }
    pub fn base(&self) -> Result<Url,Error> {
        let url=Url::parse(self.base_url.expose_to_backend()).map_err(|_|Error::new("app_login_deployment_invalid"))?;
        if url.scheme()!="https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some()
            || url.query().is_some() || url.fragment().is_some() { return Err(Error::new("app_login_deployment_invalid")); }
        Ok(url)
    }
    pub fn endpoint(&self, path: &str) -> Result<Url, Error> {
        if path.is_empty() || path.len()>256 || path.starts_with('/') || path.contains(['?', '#', ':', '%', '\\'])
            || path.split('/').any(|p| p==".." || p==".") || path.chars().any(char::is_control) {
            return Err(Error::new("app_login_deployment_invalid"));
        }
        let base=self.base()?;
        let url=base.join(path).map_err(|_|Error::new("app_login_deployment_invalid"))?;
        if url.origin()!=base.origin() { return Err(Error::new("app_login_deployment_invalid")); }
        Ok(url)
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SdkContext {
    pub application_key: Secret, pub server_address: Secret, pub device_id: Option<Secret>,
    pub selected_session_id: Option<Secret>, pub root_der_base64: Secret,
    pub client_config_decode_key: Option<Secret>, pub reference_decode_key: Option<Secret>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HostingContext {
    pub authname_prefix: Secret, pub channel_id: Secret, pub delete_int_code: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeTemplate {
    pub sdk: SdkContext, pub hosting: HostingContext, pub profile: ProfileConfig,
    pub bootstrap: Option<BootstrapConfig>, pub call_media: Option<CallMediaConfig>,
    #[serde(default)] pub cleanup_on_disconnect: bool,
    pub reconnect: Option<ReconnectPolicy>,
}
impl RuntimeTemplate {
    /// Pure construction and validation before the business request. The result
    /// stays private to the attempt and cannot be stored/started until the server
    /// has confirmed business enablement. No placeholder account credentials.
    pub fn bind(self, identity: &str, material: PhoneMaterial) -> Result<AccountConfig, Error> {
        let config=AccountConfig {
            credentials: ImportedCredentials {
                sdk: SdkCredentials { account_identity: Secret::from_owned(identity.to_owned()), phone_login_token: material.token,
                    application_key:self.sdk.application_key,server_address:self.sdk.server_address,device_id:self.sdk.device_id,
                    selected_session_id:self.sdk.selected_session_id,root_der_base64:self.sdk.root_der_base64,
                    client_config_decode_key:self.sdk.client_config_decode_key,reference_decode_key:self.sdk.reference_decode_key,
                    tls_only:true,expires_at_unix_seconds:None },
                hosting: HostingCredentials {sip_number:Secret::from_owned(identity.to_owned()),sip_password:material.sip_password,
                    authname_prefix:self.hosting.authname_prefix,channel_id:self.hosting.channel_id,delete_int_code:self.hosting.delete_int_code,
                    enrollment_confirmed:true},
            }, profile:self.profile,bootstrap:self.bootstrap,call_media:self.call_media,
            cleanup_on_disconnect:self.cleanup_on_disconnect,reconnect:self.reconnect,
        };
        Ok(config)
    }
}

pub(super) fn read_deployment(path: &Path) -> Result<Deployment,Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let file=std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path)
        .map_err(|_|Error::new("app_login_deployment_missing"))?;
    if !file.metadata().map_err(|_|Error::new("app_login_deployment_invalid"))?.is_file() { return Err(Error::new("app_login_deployment_invalid")); }
    let mut bytes=Zeroizing::new(Vec::new());
    file.take(256*1024+1).read_to_end(&mut bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
    if bytes.len()>256*1024 { return Err(Error::new("app_login_deployment_invalid")); }
    let config:Deployment=serde_json::from_slice(&bytes).map_err(|_|Error::new("app_login_deployment_invalid"))?;
    if config.version!=1 { return Err(Error::new("app_login_deployment_invalid")); }
    config.http.validate()?; Ok(config)
}
