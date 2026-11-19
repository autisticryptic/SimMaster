//! Explicit original IM business send installation, not SMS/call or user-read
//! rights. Import/start/current auth alone never submits an outbound message.
use serde::{Deserialize,Serialize};
#[derive(Clone,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImSendConfig{pub send_text:bool,pub send_info:bool,
    /// Original SDK profile Mtc.Auth.CC, not guessed DeviceInfo.CC or a number.
    /// Native missing value is the empty CountryCode notification parameter.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub auth_country_code:Option<String>,
}
impl std::fmt::Debug for NativeImSendConfig{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("NativeImSendConfig(<redacted>)")}}
impl Drop for NativeImSendConfig{fn drop(&mut self){if let Some(code)=&mut self.auth_country_code{zeroize::Zeroize::zeroize(code);}}}
impl NativeImSendConfig{pub(super) fn validate(&self)->Result<(),&'static str>{if !(self.send_text||self.send_info)||self.auth_country_code.as_ref().is_some_and(|v|v.len()>6||!v.bytes().all(|b|b.is_ascii_digit())){Err("app_account_im_send_policy_invalid")}else{Ok(())}}}
