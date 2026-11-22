//! Private current actor admission/accounting for Client-bound nested READs.
//! No outer or nested request ID is assigned here; canonical routing is unchanged.
use super::*;
impl ArcRpcSession{
    pub(in super::super) fn capture_client_user_entry_auth(&self,query:&Arc<super::super::user_entry_client::ClientUserEntryQuery>)->Result<(Box<UserEntryAuth>,Arc<super::super::native_rpc_budget::NativeRpcReservation>),super::super::user_entry_rpc::UserEntryError>{
        use super::super::user_entry_rpc::UserEntryError as E;
        if !query.matches(&self.owner,&self.connection){return Err(E::Scope);}
        // The same reservation also covers retained tracker/scheduler/actor
        // map nodes and the private request model before their first allocation.
        let bytes=self.user_entry_auth_retained_bytes().map_err(|_|E::Scope)?.checked_add(query.argument_bytes()?).and_then(|n|n.checked_add(std::mem::size_of::<super::super::user_entry_client::ClientUserEntryRequest>()+4096)).ok_or(E::Capacity)?;
        let budget=self.connection.reserve_native_rpc(bytes).map_err(|_|E::Capacity)?;
        // Reservation precedes every private UID/selector/token/argument copy.
        let auth=self.user_entry_auth().map_err(|_|E::Scope)?;
        Ok((auth,budget))
    }
    pub(in super::super) fn reserve_scheduled_client_packets(&self,batch:&super::super::batch_scheduler::ScheduledBatch,routing:&[RpcProperty<'_>])->Result<Option<Arc<super::super::native_rpc_budget::NativeRpcReservation>>,CodecError>{
        if !batch.has_client_reads(){return Ok(None);}
        if !self.native_receiver_scope()||!batch.matches(&self.owner,&self.connection){return Err(CodecError::InvalidField);}
        reject_auth_keys(routing)?;
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        let arguments=batch.encoded_argument_bytes(auth.user_id.len(),auth.selected_session_id.len())?;
        let metadata=self.account_metadata(routing,self.common_identity.as_deref())?;
        let properties=metadata.iter().try_fold(0usize,|total,p|total.checked_add(8+p.key.len()+p.value.len()).ok_or(CodecError::TooLarge))?;
        let rpc=17usize.checked_add(properties).and_then(|n|n.checked_add(8+5+auth.token.len())).and_then(|n|n.checked_add(SWAP_CALLS_METHOD.len())).and_then(|n|n.checked_add(arguments)).ok_or(CodecError::TooLarge)?;
        let fragments=(rpc+4).div_ceil(super::super::transport_packets::TLS_FRAGMENT_THRESHOLD);
        // No-growth measured writers; include the simultaneously retained RPC,
        // fragmentation staging copy, actual emitted packets, borrowed/sorted
        // models and both holder vectors. Existing128KiB is never enlarged.
        let model=batch.client_encoding_model_bytes()?.checked_add(metadata.len()*2*std::mem::size_of::<usize>()).ok_or(CodecError::TooLarge)?;
        let bytes=rpc.checked_mul(3).and_then(|n|n.checked_add(arguments)).and_then(|n|n.checked_add(fragments*128+256)).and_then(|n|n.checked_add(model)).ok_or(CodecError::TooLarge)?;
        self.connection.reserve_native_rpc(bytes).map(Some)
    }
}
