//! Purpose-specific admission. Nothing here accepts an arbitrary object, method or sequence from HTTP.
use super::*;
use std::time::Instant;

impl ScheduledBatch {
    pub(in super::super) fn alias_sequence(&self)->Option<i32>{self.alias_holder().map(|r|r.sequence())}
    pub(in super::super) fn alias_holder(&self) -> Option<Arc<AliasRead>> {
        self.bucket.requests.values().find_map(|r| r.alias.clone())
    }
}
impl BatchScheduler {
    pub(in super::super) fn expire_ue_alias(&mut self) {
        if self.alias_read.as_ref().is_some_and(|read| !read.pending()) {
            let read = self.alias_read.take().expect("checked alias interest");
            self.retire_client_read(read.sequence());
        }
    }
    pub(in super::super) fn queue_ue_alias(&mut self, origin: &Arc<AliasOrigin>, sequence: super::super::rpc_pending::UeAliasSequence, deadline: Instant) -> Result<Arc<AliasRead>, BatchError> {
        self.require_open()?;
        let sequence = sequence.get();
        if !origin.matches(&self.owner, &self.connection) { return Err(BatchError::Scope); }
        if sequence <= self.last_client_sequence { return Err(BatchError::Epoch); }
        self.expire_ue_alias();
        if self.prepared.is_some() || self.alias_read.is_some() { return Err(BatchError::Backpressure); }
        let (count, _, bytes) = self.totals();
        if count >= MAX_MAP_ENTRIES || self.pending_client.len() >= MAX_MAP_ENTRIES { return Err(BatchError::Backpressure); }
        let read = AliasRead::start(origin, sequence, deadline)?;
        let size = RequestData::measure(&read.nested())?;
        if bytes.checked_add(size).is_none_or(|total| total > MAX_RETAINED_BYTES) { return Err(BatchError::Backpressure); }
        let data = RequestData { method: vec![], arguments: vec![], metadata: vec![], bytes: size, client: None, alias: Some(read.clone()) };
        self.last_client_sequence = sequence;
        self.pending_client.insert(sequence); self.client_reads.insert(sequence);
        self.working.requests.insert(sequence, Arc::new(data)); self.alias_read = Some(read.clone());
        Ok(read)
    }
    pub(in super::super) fn finish_ue_alias(&mut self, original: &Arc<AliasOrigin>, result: ue_alias::StagedAliases) {
        if !original.matches(&self.owner, &self.connection) || !self.alias_read.as_ref().is_some_and(|r| result.belongs_to(r)) { return; }
        result.finish(original);
        self.expire_ue_alias();
    }
}
