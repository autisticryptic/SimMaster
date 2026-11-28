//! Original current Client batch READ interests. Consumer cancellation cannot
//! abort an already encoded mixed frame or loosen any WRITE retirement rule.
use super::*;
use super::super::{user_entry_client::{ClientUserEntryQuery,ClientReadWait},user_entry_rpc::{OwnedUserEntryReply,UserEntryError}};
pub(super) struct ClientReadWork{pub(super) query:Arc<ClientUserEntryQuery>,pub(super) routing:Vec<Property>,pub(super) reply:oneshot::Sender<Result<OwnedUserEntryReply,UserEntryError>>,_routing_budget:Arc<super::super::native_rpc_budget::NativeRpcReservation>}
pub(in super::super) struct ClientReadPending{pub(super) query:Arc<ClientUserEntryQuery>,pub(super) reply:oneshot::Sender<Result<OwnedUserEntryReply,UserEntryError>>}
/// Kept behind OfflineBackend's already owned container so the original Actor
/// layout, existing standard-stack literals and test definitions stay intact.
#[derive(Default)]
pub(in super::super) struct ClientReadState{pub(in super::super) client_read_pending:std::collections::BTreeMap<i32,ClientReadPending>,pub(in super::super) client_read_outer:std::collections::BTreeMap<i32,Vec<i32>>,pub(in super::super) alias:Option<Box<super::ue_alias::AliasPending>>,pub(in super::super) alias_router:Option<super::super::router_connector::RouterPolicyIdentity>}
impl ClientReadState{pub(in super::super) fn clear(&mut self){for p in self.client_read_pending.values(){p.query.interest().retire();}self.client_read_pending.clear();self.client_read_outer.clear();self.alias=None;self.alias_router=None;}}
fn mapped(error:RuntimeError)->UserEntryError{match error{RuntimeError::Timeout=>UserEntryError::Timeout,RuntimeError::Backpressure=>UserEntryError::Capacity,RuntimeError::InvalidConfiguration=>UserEntryError::Configuration,RuntimeError::Io|RuntimeError::Tls(_)=>UserEntryError::Transport,RuntimeError::Protocol=>UserEntryError::Protocol,_=>UserEntryError::Scope}}
impl RuntimeCommands{
    pub(in super::super) async fn client_user_entry(&self,query:Arc<ClientUserEntryQuery>,routing:&[RpcProperty<'_>])->Result<OwnedUserEntryReply,UserEntryError>{
        let action=query.action();let owner=action.owner();let parent=action.parent();
        if !self.accepts(owner,&parent)||!query.matches(owner,&parent){return Err(UserEntryError::Scope);}
        let bytes=routing.iter().try_fold(64usize,|n,p|n.checked_add(p.key.len()+p.value.len()).ok_or(UserEntryError::Capacity))?;
        let budget=parent.reserve_native_rpc(bytes).map_err(|_|UserEntryError::Capacity)?;
        let captured=Property::capture(routing).map_err(mapped)?;
        let(reply,result)=oneshot::channel();
        let completion=self.enqueue(owner,&parent,Operation::ClientUserEntry(Box::new(ClientReadWork{query:query.clone(),routing:captured,reply,_routing_budget:budget}))).map_err(mapped)?;
        let _wait=ClientReadWait::original(query.interest());
        completion.await.map_err(|_|UserEntryError::Scope)?.map_err(mapped)?;
        let data=result.await.map_err(|_|UserEntryError::Scope)??;
        if !data.matches_action(action){return Err(if Instant::now()>=query.deadline(){UserEntryError::Timeout}else{UserEntryError::Scope});}
        Ok(data)
    }
}
impl<I> Actor<I> where I:AsyncRead+AsyncWrite+Unpin+Send+'static{
    pub(super) fn retire_client_reads(&mut self,now:Instant){
        self.retire_alias_read(now);
        let retired=self.backend.client_read_state().client_read_pending.iter().filter_map(|(&sequence,p)|{
            if now>=p.query.deadline(){Some((sequence,UserEntryError::Timeout))}
            else if p.reply.is_closed()||!p.query.matches(&self.owner,&self.scope){Some((sequence,UserEntryError::Scope))}else{None}
        }).collect::<Vec<_>>();
        for(sequence,error)in retired{if let Some(p)=self.backend.client_read_state_mut().client_read_pending.remove(&sequence){p.query.interest().retire();self.backend.retire_client_user_entry(sequence);let _=p.reply.send(Err(error));}}
        // Retire only READ-only outer RPC interest. Mixed frames keep all of
        // their original WRITE/control ACKs, deadlines, epochs and owners.
        let finished=self.backend.client_read_state().client_read_outer.iter().filter_map(|(&outer,sequences)|{
            (!self.backend.is_pending(outer)||sequences.iter().all(|s|!self.backend.client_read_state().client_read_pending.contains_key(s)) && !self.alias_sequences_pending(sequences)).then_some(outer)
        }).collect::<Vec<_>>();
        for outer in finished{self.backend.client_read_state_mut().client_read_outer.remove(&outer);self.backend.retire_request(outer);self.pending.remove(&outer);}
    }
    pub(super) fn deliver_client_user_entry_replies(&mut self,outcome:Result<(),RuntimeError>){
        for completion in self.backend.take_client_user_entry_replies(){
            let sequence=completion.request.sequence();let Some(p)=self.backend.client_read_state_mut().client_read_pending.remove(&sequence)else{continue;};
            let result=if Instant::now()>=p.query.deadline(){Err(UserEntryError::Timeout)}
                else if p.reply.is_closed()||!Arc::ptr_eq(&p.query,completion.request.query())||!completion.request.matches(&self.owner,&self.scope){Err(UserEntryError::Scope)}
                else{outcome.map_err(mapped).and(completion.result)};
            p.query.interest().retire();let _=p.reply.send(result);
        }
        self.retire_client_reads(Instant::now());
    }
}
