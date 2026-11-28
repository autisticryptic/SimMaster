//! One original Common actor READ. Dropped interest cannot cancel a partially
//! submitted mixed frame; all original WRITE guards and physical Join survive.
use super::*;
use super::super::{batch_scheduler::AliasWait,rpc_stream::CodecError,ue_alias_action::{AliasAction,AliasActionWait,AliasObservation},ue_alias_login::AliasLogin};

pub(super) struct AliasLoginWork{binding:AliasLogin,login:Login,routing:Vec<Property>,_charge:Arc<super::super::native_rpc_budget::NativeRpcReservation>}
impl AliasLoginWork{pub(super) fn bytes(&self)->usize{self.login.identity.len()+self.login.domain.len()+self.login.selector.len()+self.login.phone.len()+self.login.key.len()+self.login.domain_id.len()+self.login.application_id.len()+self.login.cc.len()+self.login.properties.iter().chain(&self.routing).map(|p|p.key.len()+p.value.len()).sum::<usize>()}}
pub(super) struct AliasReadWork{action:Arc<AliasAction>,routing:Vec<Property>,reply:oneshot::Sender<Result<AliasObservation,RuntimeError>>,_charge:Arc<super::super::native_rpc_budget::NativeRpcReservation>}
impl AliasReadWork{pub(super) fn deadline(&self)->Instant{self.action.deadline()}
    pub(super) fn reject(self,error:RuntimeError){let _=self.reply.send(Err(error));}
    pub(super) fn bytes(&self)->usize{std::mem::size_of::<Self>()+self.routing.iter().map(|p|p.key.len()+p.value.len()).sum::<usize>()}}
pub(in super::super) struct AliasPending{wait:AliasWait,work:AliasReadWork,deadline:Instant}
fn error(code:CodecError)->RuntimeError{match code{CodecError::TooLarge=>RuntimeError::Backpressure,CodecError::InvalidField=>RuntimeError::Cancelled,_=>RuntimeError::Protocol}}

impl RuntimeCommands{
    pub(in super::super) async fn login_with_alias_source(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,binding:AliasLogin,inputs:ArcLoginInputs<'_>,routing:&[RpcProperty<'_>])->Result<RuntimeReceipt,RuntimeError>{
        if !self.accepts(owner,scope)||!binding.current(owner,scope,&inputs){return Err(RuntimeError::Scope);}
        let bytes=[inputs.typed_identity,inputs.domain_name,inputs.selected_session_id,inputs.phone_login_token,inputs.application_key,inputs.domain_id,inputs.application_id,inputs.device_info_cc].iter().try_fold(std::mem::size_of::<AliasLoginWork>()+128usize,|n,v|n.checked_add(v.len()).ok_or(RuntimeError::Backpressure))?;
        let bytes=inputs.client_properties.iter().chain(routing).try_fold(bytes,|n,p|n.checked_add(p.key.len()+p.value.len()+std::mem::size_of::<Property>()+std::mem::size_of::<RpcProperty<'_>>()).ok_or(RuntimeError::Backpressure))?;
        let charge=scope.reserve_native_rpc(bytes).map_err(|_|RuntimeError::Backpressure)?;
        let work=AliasLoginWork{binding,login:Login::capture(inputs)?,routing:Property::capture(routing)?,_charge:charge};
        self.submit(owner,scope,Operation::AliasLogin(Box::new(work))).await
    }
    pub(in super::super) async fn read_aliases(&self,action:Arc<AliasAction>,routing:&[RpcProperty<'_>])->Result<AliasObservation,RuntimeError>{
        let owner=action.owner();let scope=action.scope();
        if !self.accepts(owner,&scope)||!action.claim(){return Err(RuntimeError::Scope);}
        let mut interest=AliasActionWait::original(action.clone());
        let bytes=routing.iter().try_fold(std::mem::size_of::<AliasReadWork>()+128usize,|n,p|n.checked_add(p.key.len()+p.value.len()+std::mem::size_of::<Property>()+std::mem::size_of::<RpcProperty<'_>>()).ok_or(RuntimeError::Backpressure))?;
        let charge=scope.reserve_native_rpc(bytes).map_err(|_|RuntimeError::Backpressure)?;
        let routing=Property::capture(routing)?;let(reply,result)=oneshot::channel();
        let complete=self.enqueue(owner,&scope,Operation::ReadAliases(Box::new(AliasReadWork{action:action.clone(),routing,reply,_charge:charge})))?;
        let operation=async{complete.await.map_err(|_|RuntimeError::Closed)??;result.await.map_err(|_|RuntimeError::Closed)?};
        let observation=tokio::select!{biased;_=action.retired()=>return Err(RuntimeError::Cancelled),r=tokio::time::timeout_at(action.deadline().into(),operation)=>r.map_err(|_|RuntimeError::Timeout)??};
        if !observation.current(){return Err(if observation.expired(){RuntimeError::Timeout}else{RuntimeError::Cancelled});}interest.disarm();Ok(observation)
    }
}
impl<I> Actor<I> where I:AsyncRead+AsyncWrite+Unpin+Send+'static{
    pub(super) fn prepare_alias_login(&mut self,work:AliasLoginWork)->Result<super::super::arc_protocol::OwnedCallPackets,super::super::arc_protocol::ProtocolError>{
        if self.common.is_none(){return Err(CodecError::InvalidField.into());}
        let props=borrowed(&work.login.properties);self.backend.prepare_bound_alias_login(work.binding,work.login.inputs(&props),&borrowed(&work.routing))
    }
    pub(super) async fn prepare_alias_read(&mut self,work:AliasReadWork,native_deadline:Option<Instant>,elapsed:u64)->Result<(super::super::arc_protocol::OwnedCallPackets,Box<AliasPending>),RuntimeError>{
        let fail=|work:AliasReadWork,reason|{let _=work.reply.send(Err(reason));reason};
        if self.common.is_none()||self.backend.client_read_state().alias.is_some()||!work.action.matches(&self.owner,&self.scope)||work.reply.is_closed(){return Err(fail(work,RuntimeError::Cancelled));}
        let Some(end)=native_deadline else{return Err(fail(work,RuntimeError::InvalidConfiguration));};let end=end.min(work.action.deadline());
        let wait=match self.backend.begin_ue_alias_read(&self.owner,&self.scope,end){Ok(wait)=>wait,Err(e)=>return Err(fail(work,match e{super::super::arc_protocol::ProtocolError::Rpc(CodecError::TooLarge)|super::super::arc_protocol::ProtocolError::Batch(super::super::batch_scheduler::BatchError::Backpressure)=>RuntimeError::Backpressure,_=>RuntimeError::Scope}))};
        if !wait.attach_actor_interest(work.action.cancellation_gate(),work.action.parent_gate()){drop(wait);return Err(fail(work,RuntimeError::Cancelled));}
        #[cfg(test)]if let Some(test)=work.action.test(){test.after_admission(wait.sequence(),&self.scope).await;}
        if Instant::now()>=end{drop(wait);return Err(fail(work,RuntimeError::Timeout));}
        if !work.action.live()||work.reply.is_closed(){drop(wait);return Err(fail(work,RuntimeError::Cancelled));}
        let packets=match self.backend.prepare_swap(elapsed,&borrowed(&work.routing)){
            Ok(p)=>p,Err(e)=>{drop(wait);return Err(fail(work,match e{super::super::arc_protocol::ProtocolError::Batch(super::super::batch_scheduler::BatchError::Backpressure)=>RuntimeError::Backpressure,_=>RuntimeError::Protocol}));}
        };
        Ok((packets,Box::new(AliasPending{wait,work,deadline:end})))
    }
    #[cfg(test)] pub(super) fn alias_prefix_test(&mut self,pending:&AliasPending)->Result<(),RuntimeError>{
        if let Some(test)=pending.work.action.test(){if test.is_prefix(){self.common.as_mut().ok_or(RuntimeError::Closed)?.gate_client_read_test(test);}}Ok(())
    }
    pub(super) fn retain_alias_pending(&mut self,pending:Box<AliasPending>){self.backend.client_read_state_mut().alias=Some(pending);self.retire_alias_read(Instant::now());}
    pub(super) fn alias_sequences_pending(&self,sequences:&[i32])->bool{self.backend.client_read_state().alias.as_ref().is_some_and(|p|sequences.contains(&p.wait.sequence()))}
    pub(super) fn finish_alias_delivery(&mut self,outcome:Result<(),RuntimeError>){
        self.retire_alias_read(Instant::now());
        let Some(p)=self.backend.client_read_state().alias.as_ref()else{return;};
        if outcome.is_ok()&&p.wait.outcome()!=Some(Ok(())){return;}
        let p=self.backend.client_read_state_mut().alias.take().expect("original alias completion");
        let result=match outcome{Ok(())=>AliasObservation::original(p.wait,p.work.action.clone()).map_err(error),Err(error)=>Err(error)};
        let result=if Instant::now()>=p.deadline{Err(RuntimeError::Timeout)}else{result};
        let _=p.work.reply.send(result);self.retire_client_reads(Instant::now());
    }
    pub(super) fn retire_alias_read(&mut self,_now:Instant){
        let Some(p)=self.backend.client_read_state().alias.as_ref()else{return;};
        let state=match p.wait.sampled_outcome(){
            super::super::batch_scheduler::AliasOutcome::Expired=>Some(Err(RuntimeError::Timeout)),
            super::super::batch_scheduler::AliasOutcome::Live(value)=>{
                if p.work.reply.is_closed()||!p.work.action.matches(&self.owner,&self.scope){Some(Err(RuntimeError::Cancelled))}
                else{value.and_then(|v|v.err().map(|e|Err(error(e))))}
            }
        };
        if let Some(state)=state{let pending=self.backend.client_read_state_mut().alias.take().expect("original alias pending");
            let result=match state{Ok(())=>AliasObservation::original(pending.wait,pending.work.action.clone()).map_err(error),Err(error)=>Err(error)};
            let _=pending.work.reply.send(result);
        }
    }
}
