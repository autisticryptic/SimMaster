//! Explicit installed native IM synchronization read policy. Not current
//! authentication, a server read receipt, paid-send authority or TLS exception.
use serde::{Deserialize,Serialize};
use std::fmt;
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImSyncConfig{pub receive:bool}
impl fmt::Debug for NativeImSyncConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeImSyncConfig(<redacted>)")}}
impl NativeImSyncConfig{
    pub(super) fn validate(&self)->Result<(),&'static str>{if self.receive{Ok(())}else{Err("app_account_im_sync_policy_invalid")}}
}
