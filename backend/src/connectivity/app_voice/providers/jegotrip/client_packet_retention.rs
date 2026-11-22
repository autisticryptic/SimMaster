//! One accounting anchor shared by the ACTUAL emitted Client packet objects.
//! It carries no new IO or reply authority; bytes and private request snapshots
//! stay charged if the enclosing descriptor is taken/popped/partially moved.
use super::{native_rpc_budget::NativeRpcReservation,user_entry_client::ClientUserEntryRequest};
use std::sync::Arc;
pub(super) struct ClientPacketRetention{_requests:Vec<Arc<ClientUserEntryRequest>>,_budget:Arc<NativeRpcReservation>}
impl ClientPacketRetention{pub(super) fn original(requests:Vec<Arc<ClientUserEntryRequest>>,budget:Arc<NativeRpcReservation>)->Arc<Self>{Arc::new(Self{_requests:requests,_budget:budget})}}
