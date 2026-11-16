//! User-read installation is independent of automatic received-position
//! receipts, business sends and transport trust. Absent policy does no action.
use serde::{Deserialize,Serialize};
use std::fmt;
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeUserReadConfig{pub mark_read:bool}
impl fmt::Debug for NativeUserReadConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeUserReadConfig(<redacted>)")}}
impl NativeUserReadConfig{
    pub(super) fn validate(&self)->Result<(),&'static str>{if self.mark_read{Ok(())}else{Err("app_account_im_user_read_policy_invalid")}}
}
