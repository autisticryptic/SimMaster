//! Native direct-request magic retention: two 180-second receive windows.
//! Pending reply ownership is separate and never evicted by this history.
//! A window is not token expiry or SMS-content/remote-id deduplication.
use super::rpc_stream::CodecError;
use std::{collections::BTreeMap, time::{Duration, Instant}};
use zeroize::Zeroizing;

const WINDOW: Duration = Duration::from_secs(180);
pub(super) const MAX_HISTORY: usize = 4096;
pub(super) const MAX_MAGIC_BYTES: usize = 1024;
// Retain the existing total-byte safety policy, independently of the native
// time-window semantics. A burst can fail closed; completed lifetime count is
// no longer the same as simultaneously retained identities.
pub(super) const MAX_RETAINED_BYTES: usize = super::rpc_stream::MAX_STREAM_BYTES;

pub(super) struct DirectHistory {
    buckets: [BTreeMap<i32, Zeroizing<Vec<u8>>>; 2],
    current: usize, rotated: Instant, observed: Instant,
}
impl DirectHistory {
    pub(super) fn new(now: Instant) -> Self {
        Self { buckets: std::array::from_fn(|_| BTreeMap::new()), current: 0, rotated: now, observed: now }
    }
    pub(super) fn tick(&mut self, now: Instant) -> Result<(), CodecError> {
        if now < self.observed { return Err(CodecError::InvalidField); }
        self.observed = now;
        // Native rotates one bucket per receive-time check, strictly >180s;
        // do not invent catch-up rotations or reset an original replay scope.
        if now.duration_since(self.rotated) > WINDOW {
            self.current ^= 1; self.buckets[self.current].clear(); self.rotated = now;
        }
        Ok(())
    }
    pub(super) fn contains_id(&self, id: i32) -> bool { self.buckets.iter().any(|bucket| bucket.contains_key(&id)) }
    pub(super) fn count(&self) -> usize { self.buckets.iter().map(BTreeMap::len).sum() }
    pub(super) fn bytes(&self) -> usize { self.buckets.iter().flat_map(|bucket| bucket.values()).map(|magic| magic.len()).sum() }
    pub(super) fn refresh_duplicate(&mut self, id: i32, magic: &[u8]) -> bool {
        let previous = self.buckets.iter().enumerate().find_map(|(index, bucket)|
            bucket.iter().find(|(_, prior)| prior.as_slice() == magic).map(|(old, _)| (index, *old)));
        let Some((index, old)) = previous else { return false; };
        let retained = self.buckets[index].remove(&old).expect("found original magic identity");
        self.buckets[self.current].insert(id, retained); true
    }
    pub(super) fn record(&mut self, id: i32, magic: Vec<u8>) { self.buckets[self.current].insert(id, Zeroizing::new(magic)); }
    pub(super) fn clear(&mut self) { for bucket in &mut self.buckets { bucket.clear(); } }
}

#[cfg(test)]
#[path = "direct_history_tests.rs"]
mod tests;
