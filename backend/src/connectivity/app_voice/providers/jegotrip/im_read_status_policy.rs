//! Separately installed manual Mtc_ImSetReadStatus operation. Not automatic
//! received receipts, user UDMR/read grant or permission to start a receiver.
use serde::{Deserialize,Serialize};
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeReadStatusConfig{pub set_read_status:bool}
impl std::fmt::Debug for NativeReadStatusConfig{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("NativeReadStatusConfig(<redacted>)")}}
impl NativeReadStatusConfig{pub(super) fn validate(&self)->Result<(),&'static str>{if self.set_read_status{Ok(())}else{Err("app_account_im_read_status_policy_invalid")}}}
