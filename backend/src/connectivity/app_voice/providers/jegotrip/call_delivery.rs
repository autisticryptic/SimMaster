//! Original CallClientEx deliveries and BRANDED direct/batch reply capabilities.
//! A notice receipt is not call acceptance, SDK authentication or media-ready.
use super::{call_control::CallControlTicket,arc_protocol::DirectReplyTicket,batch_scheduler::ServerRequestTicket,
    call_binding::OwnedCallNotice,rpc_calls::{CallNoticeKind,NativeCallId},
    rpc_pending::RpcConnectionScope,rpc_stream::CodecError,session::AttemptOwner};
use std::fmt;

pub(super) enum CallReplySource { Direct(DirectReplyTicket), Batch(ServerRequestTicket) }
pub struct CallReplyTicket {
    source:CallReplySource,kind:CallNoticeKind,native_id:NativeCallId,control:Option<CallControlTicket>,
}
impl fmt::Debug for CallReplyTicket{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CallReplyTicket(<redacted>)")}}
impl CallReplyTicket{
    pub fn kind(&self)->CallNoticeKind{self.kind}
    pub fn native_id(&self)->NativeCallId{self.native_id}
    pub fn is_open(&self)->bool{match &self.source{CallReplySource::Direct(ticket)=>ticket.is_open(),CallReplySource::Batch(ticket)=>ticket.is_open()}}
    pub fn control_ticket(&self)->Option<&CallControlTicket>{self.control.as_ref()}
    pub(super) fn source(&self)->&CallReplySource{&self.source}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        match &self.source{CallReplySource::Direct(ticket)=>ticket.matches(owner,scope),CallReplySource::Batch(ticket)=>ticket.matches(owner,scope)}
    }
    pub(super) fn captured_copy(&self)->Self{
        let source=match &self.source{CallReplySource::Direct(ticket)=>CallReplySource::Direct(ticket.captured_copy()),CallReplySource::Batch(ticket)=>CallReplySource::Batch(ticket.captured_copy())};
        Self{source,kind:self.kind,native_id:self.native_id,control:self.control.as_ref().map(CallControlTicket::captured_copy)}
    }
}
pub struct IncomingCallDelivery{notice:OwnedCallNotice,ticket:CallReplyTicket}
impl fmt::Debug for IncomingCallDelivery{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("IncomingCallDelivery(<redacted>)")}}
impl IncomingCallDelivery{
    pub fn owner(&self)->&AttemptOwner{self.notice.owner()}
    pub fn connection_scope(&self)->RpcConnectionScope{self.notice.connection_scope()}
    pub fn notice(&self)->&OwnedCallNotice{&self.notice}
    pub fn ticket(&self)->&CallReplyTicket{&self.ticket}
    pub fn control_ticket(&self)->Option<&CallControlTicket>{self.ticket.control_ticket()}
    pub(super) fn with_control(mut self,control:Option<CallControlTicket>)->Result<Self,CodecError>{self.attach_control(control)?;Ok(self)}
    pub(super) fn attach_control(&mut self,control:Option<CallControlTicket>)->Result<(),CodecError>{
        if control.as_ref().is_some_and(|ticket|!ticket.matches(self.notice.owner(),&self.notice.connection_scope())||ticket.native_id()!=self.notice.native_id()){return Err(CodecError::InvalidField);}
        self.ticket.control=control;Ok(())
    }
    pub(super) fn from_direct(notice:OwnedCallNotice,ticket:DirectReplyTicket)->Result<Self,CodecError>{Self::capture(notice,CallReplySource::Direct(ticket))}
    pub(super) fn from_batch(notice:OwnedCallNotice,ticket:ServerRequestTicket)->Result<Self,CodecError>{Self::capture(notice,CallReplySource::Batch(ticket))}
    fn capture(notice:OwnedCallNotice,source:CallReplySource)->Result<Self,CodecError>{
        let original_scope=notice.connection_scope();
        let matches=match &source{CallReplySource::Direct(ticket)=>ticket.matches(notice.owner(),&original_scope),CallReplySource::Batch(ticket)=>ticket.matches(notice.owner(),&original_scope)};
        if !matches || !notice.matches(notice.owner(),&original_scope){return Err(CodecError::InvalidField);}
        let ticket=CallReplyTicket{source,kind:notice.kind(),native_id:notice.native_id(),control:None};Ok(Self{notice,ticket})
    }
}
