//! Explicit received-position write installation. Separate from readonly
//! synchronization, user read/open intent, business sends and transport trust.
use serde::{Deserialize,Serialize};
use std::fmt;
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImReceiptConfig{pub set_received:bool}
impl fmt::Debug for NativeImReceiptConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeImReceiptConfig(<redacted>)")}}
impl NativeImReceiptConfig{
    pub(super) fn validate(&self)->Result<(),&'static str>{
        if self.set_received{Ok(())}else{Err("app_account_im_receipt_policy_invalid")}
    }
}
