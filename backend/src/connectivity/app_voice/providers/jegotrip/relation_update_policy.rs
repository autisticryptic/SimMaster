//! Separate local installation policy for Group/Buddy relation WRITEs.
//! Directory READ, IM-send rights, login and cached relation DATA are not this
//! permission. None is the legacy default; explicit user consent/nonce/current
//! original SDK/SQL/run checks remain necessary at each later action boundary.
use serde::{Deserialize,Serialize};
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRelationUpdateConfig{pub additions:bool,pub updates:bool,pub removals:bool}
impl std::fmt::Debug for NativeRelationUpdateConfig{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("NativeRelationUpdateConfig(<redacted>)")}}
impl NativeRelationUpdateConfig{
    pub(super) fn validate(self)->Result<(),&'static str>{if !(self.additions||self.updates||self.removals){Err("app_account_relation_policy_invalid")}else{Ok(())}}
    pub(super) fn admits(self,adds:usize,updates:usize,removes:usize)->bool{
        (adds>0||updates>0||removes>0)&&(adds==0||self.additions)&&(updates==0||self.updates)&&(removes==0||self.removals)
    }
}
#[cfg(test)]
#[path="relation_update_policy_tests.rs"]
mod tests;
