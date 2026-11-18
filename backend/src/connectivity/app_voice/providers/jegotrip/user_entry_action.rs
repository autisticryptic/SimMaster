//! One explicitly consented lookup intent; account scope, not an SDK identity.
use super::{im_inbox::ImReceiptGate,rpc_pending::RpcConnectionScope,session::AttemptOwner,user_entry_data::RecipientUri};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
pub fn valid_user_entry_recipient(recipient:&str)->bool{RecipientUri::parse(recipient.as_bytes()).is_ok()}
pub fn valid_user_read_recipient(recipient:&str)->bool{RecipientUri::parse(recipient.as_bytes()).is_ok_and(|uri|uri.kind()!=super::user_entry_data::AidKind::Uid)}
pub(super) struct UserEntryAction{owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:Zeroizing<Vec<u8>>,
    #[cfg(test)]auxiliary_io:std::sync::Mutex<Option<Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>>>,
}
impl fmt::Debug for UserEntryAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryAction(<redacted>)")}}
impl UserEntryAction{
    pub(super) fn original(owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:&[u8])->Result<Arc<Self>,&'static str>{
        RecipientUri::parse(recipient).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        if !parent.is_open()||parent.session_revoked()||!gate.live()||Instant::now()>=deadline{return Err("app_account_user_entry_scope_invalid");}
        Ok(Arc::new(Self{owner,parent,gate,deadline,recipient:Zeroizing::new(recipient.to_vec()),
            #[cfg(test)]auxiliary_io:std::sync::Mutex::new(None),
        }))
    }
    pub(super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(super) fn parent(&self)->RpcConnectionScope{self.parent.clone()}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn recipient(&self)->&[u8]{&self.recipient}
    pub(super) fn live(&self)->bool{self.parent.is_open()&&!self.parent.session_revoked()&&self.gate.live()&&Instant::now()<self.deadline}
    #[cfg(test)]pub(super) fn set_auxiliary_io_test(&self,test:Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>){*self.auxiliary_io.lock().unwrap()=Some(test);}
    #[cfg(test)]pub(super) fn auxiliary_io_test(&self)->Option<Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>>{self.auxiliary_io.lock().unwrap().clone()}
}
#[derive(serde::Serialize)]
pub struct UserEntryObservation{pub kind:&'static str,pub report:&'static str,pub observed_at:String}
impl fmt::Debug for UserEntryObservation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryObservation(<redacted>)")}}
