//! One explicitly consented lookup intent; account scope, not an SDK identity.
use super::{im_inbox::ImReceiptGate,rpc_pending::RpcConnectionScope,session::AttemptOwner,user_entry_data::RecipientUri};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
pub fn valid_user_entry_recipient(recipient:&str)->bool{RecipientUri::parse(recipient.as_bytes()).is_ok()}
pub fn valid_user_read_recipient(recipient:&str)->bool{RecipientUri::parse(recipient.as_bytes()).is_ok_and(|uri|uri.kind()!=super::user_entry_data::AidKind::Uid)}
pub(super) struct UserEntryAction{owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:Zeroizing<Vec<u8>>,
    input:Option<Arc<super::native_rpc_budget::NativeRpcReservation>>,
    #[cfg(test)]auxiliary_io:std::sync::Mutex<Option<Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>>>,
    #[cfg(test)]client_io:std::sync::Mutex<Option<Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>>>,
}
impl fmt::Debug for UserEntryAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryAction(<redacted>)")}}
impl UserEntryAction{
    pub(super) fn original(owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:&[u8])->Result<Arc<Self>,&'static str>{
        Self::captured(owner,parent,gate,deadline,recipient,None)
    }
    /// The manager's synchronously reserved input keeps the original budget
    /// through every pending/result holder. It is accounting, not a query grant.
    pub(super) fn reserved(owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:&[u8],input:Arc<super::native_rpc_budget::NativeRpcReservation>)->Result<Arc<Self>,&'static str>{
        if !input.belongs_to(&parent){return Err("app_account_user_entry_scope_invalid");}
        Self::captured(owner,parent,gate,deadline,recipient,Some(input))
    }
    fn captured(owner:AttemptOwner,parent:RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant,recipient:&[u8],input:Option<Arc<super::native_rpc_budget::NativeRpcReservation>>)->Result<Arc<Self>,&'static str>{
        RecipientUri::parse(recipient).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        if !parent.is_open()||parent.session_revoked()||!gate.live()||Instant::now()>=deadline{return Err("app_account_user_entry_scope_invalid");}
        Ok(Arc::new(Self{owner,parent,gate,deadline,recipient:Zeroizing::new(recipient.to_vec()),input,
            #[cfg(test)]auxiliary_io:std::sync::Mutex::new(None),
            #[cfg(test)]client_io:std::sync::Mutex::new(None),
        }))
    }
    pub(super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(super) fn parent(&self)->RpcConnectionScope{self.parent.clone()}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn recipient(&self)->&[u8]{&self.recipient}
    pub(super) fn live(&self)->bool{self.parent.is_open()&&!self.parent.session_revoked()&&self.gate.live()&&Instant::now()<self.deadline&&self.input.as_ref().is_none_or(|input|input.belongs_to(&self.parent))}
    #[cfg(test)]pub(super) fn set_auxiliary_io_test(&self,test:Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>){*self.auxiliary_io.lock().unwrap()=Some(test);}
    #[cfg(test)]pub(super) fn auxiliary_io_test(&self)->Option<Arc<super::router_runtime::tests::auxiliary_io::AuxiliaryIoTest>>{self.auxiliary_io.lock().unwrap().clone()}
    #[cfg(test)]pub(super) fn set_client_io_test(&self,test:Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>){assert!(self.client_io.lock().unwrap().replace(test).is_none());}
    #[cfg(test)]pub(super) fn client_io_test(&self)->Option<Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>>{self.client_io.lock().unwrap().clone()}
}
#[derive(serde::Serialize)]
pub struct UserEntryObservation{pub kind:&'static str,pub report:&'static str,pub observed_at:String}
impl fmt::Debug for UserEntryObservation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryObservation(<redacted>)")}}
