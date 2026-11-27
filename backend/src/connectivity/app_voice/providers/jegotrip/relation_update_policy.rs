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
/// Either permission family, never a conversion between them. Runtime selects
/// it from the frozen original profile; legacy call sites remain unchanged.
#[derive(Clone,Copy)]
pub(super) enum RelationWriteGrant{Members(NativeRelationUpdateConfig),Management(super::group_management::NativeGroupManagementConfig),Creation(super::group_create::NativeGroupCreateConfig),OwnerOnly(super::group_create::NativeGroupCreateConfig)}
impl From<NativeRelationUpdateConfig> for RelationWriteGrant{fn from(p:NativeRelationUpdateConfig)->Self{Self::Members(p)}}
impl From<super::group_management::NativeGroupManagementConfig> for RelationWriteGrant{fn from(p:super::group_management::NativeGroupManagementConfig)->Self{Self::Management(p)}}
impl From<super::group_create::NativeGroupCreateConfig> for RelationWriteGrant{fn from(p:super::group_create::NativeGroupCreateConfig)->Self{Self::Creation(p)}}
impl RelationWriteGrant{pub(super) fn admits(self,counts:[usize;3],op:Option<super::group_management_store::Descriptor>)->bool{match(self,op){(Self::Members(p),None)=>p.admits(counts[0],counts[1],counts[2]),(Self::Management(p),Some(d))=>counts==[0;3]&&p.admits(d.operation),(Self::Creation(p),Some(d))=>counts==[0;3]&&d.operation==super::group_management::GroupWriteOperation::Create&&p.create,(Self::OwnerOnly(p),Some(d))=>counts==[0;3]&&d.operation==super::group_management::GroupWriteOperation::CreateOwnerOnly&&p.create,_=>false}}}
#[cfg(test)]
#[path="relation_update_policy_tests.rs"]
mod tests;
