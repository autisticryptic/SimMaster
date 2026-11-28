//! Internal protocol layer only. Ordinary account start, HTTP and group creation
//! do not install this source, send this READ, or consume membership for a WRITE.
use super::*;
use super::super::{batch_scheduler::{AliasOrigin, AliasWait}, im_inbox::ImReceiptGate,
    router_connector::RouterPolicy, runtime_provision::ProvisionedCommonLogin};
use std::{sync::Arc, time::Instant};

impl OfflineBackend {
    pub(in super::super) fn prepare_login_with_ue_alias_source(&mut self, plan: &ProvisionedCommonLogin, router: &RouterPolicy,
        gate: Arc<ImReceiptGate>, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        let owner = self.tracker.batch_owner()?;
        let scope = self.connection_scope();
        let entry = plan.entry_for(&owner, &scope, router).map_err(|_| CodecError::InvalidField)?;
        if self.ue_origin.is_some() || !self.protocol.initial_common_object_is(entry.identity()) { return Err(CodecError::InvalidField.into()); }
        let source = AliasOrigin::from_plan(&owner, &scope, router, plan, &inputs, gate)?;
        let packet = self.prepare_login(inputs, routing)?;
        self.ue_origin = Some(source);
        Ok(packet)
    }
    pub(in super::super) fn bind_alias_transport(&mut self,router:&RouterPolicy)->Result<(),ProtocolError>{
        if self.phase()!=ArcPhase::Initial||self.client_actor.alias_router.is_some(){return Err(CodecError::InvalidField.into());}
        self.client_actor.alias_router=Some(router.original_identity());Ok(())
    }
    pub(in super::super) fn prepare_bound_alias_login(&mut self, binding: super::super::ue_alias_login::AliasLogin,
        inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        let owner=self.tracker.batch_owner()?;let scope=self.connection_scope();
        if self.ue_origin.is_some()||self.client_actor.alias_router.as_ref().is_none_or(|r|!binding.transport_matches(r))||!binding.current(&owner,&scope,&inputs)||!self.protocol.initial_common_object_is(binding.entry()){return Err(CodecError::InvalidField.into());}
        let packets=self.prepare_login(inputs,routing)?;self.ue_origin=Some(binding.into_origin());Ok(packets)
    }
    pub(in super::super) fn begin_ue_alias_read(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, deadline: Instant) -> Result<AliasWait, ProtocolError> {
        let source = self.ue_origin.as_ref().ok_or(CodecError::InvalidField)?;
        if self.phase() != ArcPhase::Authenticated || !source.matches(owner, scope) { return Err(CodecError::InvalidField.into()); }
        Ok(self.tracker.queue_ue_alias(self.scheduler.as_mut().ok_or(CodecError::InvalidField)?, source, deadline)?)
    }
    // None is unknown; Some(false) requires a matched complete snapshot. This
    // tests opaque pair membership ONLY, never whether a URI's realm is current.
    pub(in super::super) fn reported_ue_pair(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, kind: &[u8], value: &[u8]) -> Option<bool> {
        let source = self.ue_origin.as_ref()?;
        if self.phase() != ArcPhase::Authenticated || !source.matches(owner, scope) { return None; }
        source.reported_pair(kind, value)
    }
    // Future preempt delivery must first verify the actual event producer.
    // This primitive cannot add rows, refresh a timeout or create a snapshot.
    pub(in super::super) fn preempt_ue_pair(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, kind: &[u8], value: &[u8]) {
        if let Some(source) = &self.ue_origin { if source.matches(owner, scope) { source.preempt(kind, value); } }
        if let Some(scheduler) = &mut self.scheduler { scheduler.expire_ue_alias(); }
    }
    pub(in super::super) fn invalidate_ue_context(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope) {
        if self.ue_origin.as_ref().is_some_and(|source| source.matches(owner, scope)) { self.retire_ue_origin(); }
    }
    // The actor can receive multiple calls together. Do not publish the first
    // alias result until every call in the same admitted group has succeeded.
    pub(super) fn with_alias_group<F>(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, receive: F) -> Result<Vec<OfflineEvent>, ProtocolError>
    where F: FnOnce(&mut Self, &mut Option<Box<super::super::batch_scheduler::StagedAliases>>) -> Result<Vec<OfflineEvent>, ProtocolError> {
        let mut staged = None;
        let events = receive(self, &mut staged);
        if let Some(result) = staged {
            if events.is_ok() && self.phase() == ArcPhase::Authenticated && self.ue_origin.as_ref().is_some_and(|o| o.matches(owner, scope)) {
                if let (Some(origin), Some(scheduler)) = (&self.ue_origin, &mut self.scheduler) { scheduler.finish_ue_alias(origin, *result); }
            } else {
                result.discard();
                if let Some(scheduler) = &mut self.scheduler { scheduler.expire_ue_alias(); }
            }
        }
        events
    }
    pub(super) fn retire_ue_origin(&mut self) {
        if let Some(source) = self.ue_origin.take() { source.retire(); }
        if let Some(scheduler) = &mut self.scheduler { scheduler.expire_ue_alias(); }
    }
}
