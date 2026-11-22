//! Original endpoint-less Client UserEntry READ inside swapCalls. The target
//! remains DATA; this purpose/action/current actor auth is not a WRITE grant.
use super::{arc_session::UserEntryAuth,rpc_batch::BatchRequest,rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError,RpcBytes,RpcProperty},session::AttemptOwner,user_entry_action::UserEntryAction,
    user_entry_client_target::NewClientTarget,user_entry_data::{self,RecipientUri,AidKind},user_entry_rpc::UserEntryError,
    native_rpc_budget::NativeRpcReservation,router_connector::RouterPolicy};
use std::{fmt,sync::{Arc,atomic::{AtomicBool,Ordering}},time::Instant};
use zeroize::Zeroizing;
/// READ interest is independent of a mixed swapCalls frame's lifetime. Dropping
/// it never cancels a partially written frame, ACKs work, or closes the SDK.
pub(super) struct ClientReadInterest{retired:AtomicBool}
impl ClientReadInterest{
    fn original()->Arc<Self>{Arc::new(Self{retired:AtomicBool::new(false)})}
    pub(super) fn retire(&self){self.retired.store(true,Ordering::SeqCst);}
    pub(super) fn is_retired(&self)->bool{self.retired.load(Ordering::SeqCst)}
}
pub(super) struct ClientReadWait{interest:Arc<ClientReadInterest>}
impl ClientReadWait{pub(super) fn original(interest:Arc<ClientReadInterest>)->Self{Self{interest}}}
impl Drop for ClientReadWait{fn drop(&mut self){self.interest.retire();}}
pub(super) struct ClientUserEntryQuery{action:Arc<UserEntryAction>,target:Arc<NewClientTarget>,interest:Arc<ClientReadInterest>,factory:Zeroizing<Vec<u8>>,cc:Zeroizing<Vec<u8>>,budget:Arc<NativeRpcReservation>}
impl fmt::Debug for ClientUserEntryQuery{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ClientUserEntryQuery(<redacted>)")}}
impl ClientUserEntryQuery{
    pub(super) fn original(action:Arc<UserEntryAction>,target:Arc<NewClientTarget>,router:&RouterPolicy)->Result<Arc<Self>,UserEntryError>{
        let owner=action.owner();let parent=action.parent();
        if !action.live(){return Err(UserEntryError::Scope);}
        let uri=RecipientUri::parse(action.recipient()).map_err(|_|UserEntryError::Configuration)?;
        if uri.kind()==AidKind::Uid{return Err(UserEntryError::Configuration);}
        let realm=target.realm_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
        if uri.authority().is_some_and(|value|value!=realm){return Err(UserEntryError::Realm);}
        let factory=target.factory_input_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
        let cc=target.cc_for(owner,&parent,router).ok_or(UserEntryError::Scope)?;
        // Reserve before the exact factory/CC copy and any actor/auth await.
        // This prefix is a separate holder from the one-shot prepared request.
        let budget=parent.reserve_native_rpc(target.retained_bytes()+factory.len()+cc.len()+action.recipient().len()+128).map_err(|_|UserEntryError::Capacity)?;
        let factory=Zeroizing::new(factory.to_vec());let cc=Zeroizing::new(cc.to_vec());
        Ok(Arc::new(Self{action,target,interest:ClientReadInterest::original(),budget,factory,cc}))
    }
    pub(super) fn action(&self)->&Arc<UserEntryAction>{&self.action}
    pub(super) fn interest(&self)->Arc<ClientReadInterest>{self.interest.clone()}
    pub(super) fn deadline(&self)->Instant{self.action.deadline()}
    pub(super) fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->bool{
        self.action.owner()==owner&&self.action.parent().matches(parent)&&self.action.live()&&!self.interest.is_retired()&&self.budget.belongs_to(parent)
    }
    pub(super) fn retained_bytes(&self)->usize{self.target.retained_bytes()+self.factory.len()+self.cc.len()+self.action.recipient().len()+128}
    pub(super) fn argument_bytes(&self)->Result<usize,UserEntryError>{let uri=RecipientUri::parse(self.action.recipient()).map_err(|_|UserEntryError::Configuration)?;super::client_user_entry_encoding::aid_arguments_bytes(uri.aid()).map_err(|_|UserEntryError::Configuration)}
}
pub(super) struct ClientUserEntryRequest{sequence:i32,query:Arc<ClientUserEntryQuery>,auth:Box<UserEntryAuth>,arguments:RpcBytes,budget:Arc<NativeRpcReservation>}
impl fmt::Debug for ClientUserEntryRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ClientUserEntryRequest(<redacted>)")}}
impl ClientUserEntryRequest{
    /// The original actor reserves authentication and argument copies BEFORE
    /// capture; only hosting's one-time nested tracker chooses the sequence.
    pub(super) fn original(sequence:i32,query:Arc<ClientUserEntryQuery>,auth:Box<UserEntryAuth>,budget:Arc<NativeRpcReservation>)->Result<Arc<Self>,UserEntryError>{
        let action=query.action();let parent=action.parent();
        if sequence<=0||!query.matches(action.owner(),&parent)||!auth.matches(action.owner(),&parent)||!budget.belongs_to(&parent){return Err(UserEntryError::Scope);}
        let uri=RecipientUri::parse(action.recipient()).map_err(|_|UserEntryError::Configuration)?;
        let arguments=super::client_user_entry_encoding::encode_aid_arguments(uri.aid()).map_err(|_|UserEntryError::Configuration)?;
        Ok(Arc::new(Self{sequence,query,auth,arguments,budget}))
    }
    pub(super) fn sequence(&self)->i32{self.sequence}
    pub(super) fn query(&self)->&Arc<ClientUserEntryQuery>{&self.query}
    pub(super) fn budget(&self)->Arc<NativeRpcReservation>{self.budget.clone()}
    pub(super) fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->bool{self.query.matches(owner,parent)&&self.auth.matches(owner,parent)&&self.budget.belongs_to(parent)}
    pub(super) fn as_batch_request(&self)->BatchRequest<'_>{
        // 593F20 gives the Agent DeviceInfo.CC; 599B38/59994C merges that
        // context, then forces whole factory-input id. No inner account-id,
        // pwd/token/locator defaults are invented. Token is the OUTER SDK call.
        BatchRequest{method:user_entry_data::AID_TO_UID_METHOD.as_bytes(),arguments:self.arguments.expose_to_transport(),metadata:vec![RpcProperty{key:b"DeviceInfo.CC",value:&self.query.cc},RpcProperty{key:b"id",value:&self.query.factory}]}
    }
    pub(super) fn retired(&self)->bool{!self.query.matches(self.query.action.owner(),&self.query.action.parent())}
    pub(super) fn retain_reply(&self,bytes:usize)->Result<(),CodecError>{self.budget.retain_reply(bytes)}
}
pub(super) struct ClientUserEntryCompletion{pub(super) request:Arc<ClientUserEntryRequest>,pub(super) result:Result<super::user_entry_rpc::OwnedUserEntryReply,UserEntryError>}
