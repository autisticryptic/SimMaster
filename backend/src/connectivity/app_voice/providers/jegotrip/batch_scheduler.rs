//! Bounded offline ARC swapCalls epochs, ACKs and server-request deduplication.
//!
//! No timer task, socket, route, login renewal or readiness is created here.
//! Six words were cross-checked against native send/reset/receive, not guessed.
//! We deliberately reject peer incarnation changes/wrap and ignore old request
//! sequences instead of resetting dedup and replaying business work. Native
//! retention pressure drops history; this adapter fails closed rather than
//! silently losing unacknowledged work or reporting it completed.
use super::{
    arc_session::OwnedBatchReply,
    rpc_batch::{BatchPayload, BatchRequest, BatchResult},
    rpc_pending::{PreparedHostingCall, RpcConnectionScope},
    rpc_stream::{CodecError, MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES},
    session::AttemptOwner,
};
use std::{collections::{BTreeMap, BTreeSet}, fmt, sync::{atomic::{AtomicBool, Ordering}, Arc}};

pub const MAX_BATCH_EPOCHS: usize = 32;
pub const BATCH_RETENTION_MS: u64 = 45_000;
pub const REQUEST_DEDUP_BITS: usize = 2048;
/// Local aggregate retained-payload policy, not a negotiated server limit.
pub const MAX_RETAINED_BYTES: usize = MAX_STREAM_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchError { Codec(CodecError), Scope, Closed, Epoch, Retention, Backpressure, Clock }

impl From<CodecError> for BatchError { fn from(value: CodecError) -> Self { Self::Codec(value) } }

impl BatchError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Codec(value) => value.code(), Self::Scope => "jegotrip_batch_scope_invalid",
            Self::Closed => "jegotrip_batch_closed", Self::Epoch => "jegotrip_batch_epoch_invalid",
            Self::Retention => "jegotrip_batch_unacked_retention", Self::Backpressure => "jegotrip_batch_backpressure",
            Self::Clock => "jegotrip_batch_clock_invalid",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchWords {
    pub incarnation: u32,
    pub next_batch: u32,
    pub batches: u32,
    pub ack_incarnation: u32,
    pub ack_next_batch: u32,
    pub ack_batches: u32,
}

impl BatchWords {
    pub fn from_fields(value: [u32; 6]) -> Self {
        Self { incarnation: value[0], next_batch: value[1], batches: value[2],
            ack_incarnation: value[3], ack_next_batch: value[4], ack_batches: value[5] }
    }
    pub fn fields(self) -> [u32; 6] {
        [self.incarnation, self.next_batch, self.batches, self.ack_incarnation, self.ack_next_batch, self.ack_batches]
    }
}

#[derive(Clone)]
struct RequestData { method: Vec<u8>, arguments: Vec<u8>, metadata: Vec<(Vec<u8>, Vec<u8>)>, bytes: usize }

impl RequestData {
    fn measure(request: &BatchRequest<'_>) -> Result<usize, CodecError> {
        if request.method.is_empty() { return Err(CodecError::InvalidField); }
        if request.method.len() > MAX_FIELD_BYTES || request.arguments.len() > MAX_FIELD_BYTES
            || request.metadata.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let mut bytes = 16usize.checked_add(request.method.len()).and_then(|n| n.checked_add(request.arguments.len()))
            .ok_or(CodecError::TooLarge)?;
        for (index, property) in request.metadata.iter().enumerate() {
            if property.key.len() > MAX_FIELD_BYTES || property.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
            if request.metadata[..index].iter().any(|p| p.key == property.key) { return Err(CodecError::DuplicateKey); }
            bytes = bytes.checked_add(8 + property.key.len() + property.value.len()).ok_or(CodecError::TooLarge)?;
        }
        if bytes > MAX_RETAINED_BYTES { return Err(CodecError::TooLarge); }
        Ok(bytes)
    }
    fn capture(request: &BatchRequest<'_>) -> Result<Self, CodecError> {
        let bytes = Self::measure(request)?;
        Ok(Self { method: request.method.to_vec(), arguments: request.arguments.to_vec(),
            metadata: request.metadata.iter().map(|p| (p.key.to_vec(), p.value.to_vec())).collect(), bytes })
    }
    fn borrowed(&self) -> BatchRequest<'_> {
        BatchRequest { method: &self.method, arguments: &self.arguments,
            metadata: self.metadata.iter().map(|(key, value)| super::rpc_stream::RpcProperty { key, value }).collect() }
    }
}

#[derive(Clone)]
struct ResultData { code: u32, stream: Vec<u8> }

impl ResultData {
    fn borrowed(&self) -> BatchResult<'_> { BatchResult { result_code: self.code, result_stream: &self.stream } }
    fn bytes(&self) -> usize { 12 + self.stream.len() }
}

#[derive(Clone, Default)]
struct Bucket { requests: BTreeMap<i32, Arc<RequestData>>, results: BTreeMap<i32, Arc<ResultData>> }

struct Epoch { created_at_ms: u64, bucket: Bucket }

/// Recovering all native peer reset/wrap policies is separate from this safe
/// offline subset. A prepared epoch snapshot remains owned through encoding.
pub struct ScheduledBatch {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    scheduler: Arc<()>,
    active: Arc<AtomicBool>,
    encoded: Arc<AtomicBool>,
    serial: u64,
    words: BatchWords,
    bucket: Bucket,
}

impl ScheduledBatch {
    pub fn words(&self) -> BatchWords { self.words }
    pub fn payload(&self) -> BatchPayload<'_> {
        BatchPayload { fields: self.words.fields(),
            requests: self.bucket.requests.iter().map(|(&s, r)| (s, r.borrowed())).collect(),
            results: self.bucket.results.iter().map(|(&s, r)| (s, r.borrowed())).collect() }
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(connection) && self.connection.is_open()
            && self.active.load(Ordering::Acquire)
    }
    pub(super) fn identity(&self) -> Arc<()> { self.scheduler.clone() }
    pub(super) fn is_encoded(&self) -> bool { self.encoded.load(Ordering::Acquire) }
    pub(super) fn mark_encoded(&self) { self.encoded.store(true, Ordering::Release); }
}

impl fmt::Debug for ScheduledBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScheduledBatch").field("requests", &self.bucket.requests.len())
            .field("results", &self.bucket.results.len()).field("contents", &"<redacted>").finish()
    }
}

pub struct ServerRequestTicket {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    scheduler: Arc<()>,
    peer_incarnation: u32,
    sequence: i32,
}

impl ServerRequestTicket {
    pub fn sequence(&self) -> i32 { self.sequence }
    pub fn owner(&self) -> &AttemptOwner { &self.owner }
    pub fn is_open(&self) -> bool { self.connection.is_open() }
    /// Private duplication for a queued original-runtime completion only. The
    /// scheduler's pending ticket map still permits exactly one completion.
    pub(super) fn captured_copy(&self) -> Self {
        Self { owner: self.owner.clone(), connection: self.connection.clone(), scheduler: self.scheduler.clone(),
            peer_incarnation: self.peer_incarnation, sequence: self.sequence }
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(connection) && self.connection.is_open()
    }
}

impl fmt::Debug for ServerRequestTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ServerRequestTicket(<redacted>)") }
}

pub struct IncomingBatchRequest { ticket: ServerRequestTicket, request: RequestData }

impl IncomingBatchRequest {
    pub fn ticket(&self) -> &ServerRequestTicket { &self.ticket }
    pub fn into_ticket(self) -> ServerRequestTicket { self.ticket }
    pub fn request(&self) -> BatchRequest<'_> { self.request.borrowed() }
}

impl fmt::Debug for IncomingBatchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingBatchRequest(<redacted>)") }
}

pub struct BatchDelivery {
    pub requests: Vec<IncomingBatchRequest>,
    pub matched_result_sequences: Vec<i32>,
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    results: BTreeMap<i32, ResultData>,
}

impl BatchDelivery {
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(connection) && self.connection.is_open()
    }
    pub(super) fn results(&self) -> impl Iterator<Item = (i32, BatchResult<'_>)> {
        self.results.iter().map(|(&sequence, result)| (sequence, result.borrowed()))
    }
}

impl fmt::Debug for BatchDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchDelivery").field("request_count", &self.requests.len())
            .field("result_count", &self.matched_result_sequences.len()).finish()
    }
}

#[derive(Clone)]
struct DedupWindow { base: u32, head: usize, bits: [u32; REQUEST_DEDUP_BITS / 32] }

impl Default for DedupWindow {
    fn default() -> Self { Self { base: 0, head: 0, bits: [0; REQUEST_DEDUP_BITS / 32] } }
}

impl DedupWindow {
    fn insert(&mut self, sequence: u32) -> bool {
        // Native unsigned-negative delta resets its ring; our safe subset must
        // never turn old/wrapped input into fresh business delivery.
        if sequence < self.base { return false; }
        let mut delta = sequence - self.base;
        if delta > 4095 {
            self.bits.fill(0); self.head = 0; self.base = sequence - 2016; delta = 2016;
        } else if delta >= REQUEST_DEDUP_BITS as u32 {
            let shift = ((delta - REQUEST_DEDUP_BITS as u32) / 32 + 1) as usize;
            for _ in 0..shift { self.bits[self.head] = 0; self.head = (self.head + 1) & 63; }
            self.base += shift as u32 * 32; delta -= shift as u32 * 32;
        }
        let slot = (self.head + (delta / 32) as usize) & 63;
        let bit = 1u32 << (delta & 31);
        let fresh = self.bits[slot] & bit == 0;
        self.bits[slot] |= bit;
        fresh
    }
}

pub struct BatchScheduler {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    identity: Arc<()>,
    local_incarnation: u32,
    next_batch: u32,
    next_serial: Option<u64>,
    prepared: Option<(u64, Arc<AtomicBool>, Arc<AtomicBool>)>,
    clock_ms: u64,
    history: BTreeMap<u32, Epoch>,
    working: Bucket,
    pending_client: BTreeSet<i32>,
    submitted_client: BTreeSet<i32>,
    last_client_sequence: i32,
    pending_server: BTreeMap<i32, usize>,
    peer_incarnation: Option<u32>,
    peer_next: u32,
    peer_batches: u32,
    dedup: DedupWindow,
    closed: bool,
}

impl fmt::Debug for BatchScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchScheduler").field("closed", &self.closed).field("history_count", &self.history.len())
            .field("client_pending", &self.pending_client.len()).field("server_pending", &self.pending_server.len()).finish()
    }
}

impl BatchScheduler {
    pub(super) fn for_arc_session(owner: AttemptOwner, connection: RpcConnectionScope, incarnation: u32)
        -> Result<Self, BatchError> {
        if incarnation == 0 { return Err(BatchError::Epoch); }
        if !connection.is_open() { return Err(BatchError::Closed); }
        Ok(Self { owner, connection, identity: Arc::new(()), local_incarnation: incarnation, next_batch: 0,
            next_serial: Some(1), prepared: None, clock_ms: 0, history: BTreeMap::new(), working: Bucket::default(),
            pending_client: BTreeSet::new(), submitted_client: BTreeSet::new(),
            last_client_sequence: 0, pending_server: BTreeMap::new(),
            peer_incarnation: None, peer_next: 0, peer_batches: 0, dedup: DedupWindow::default(), closed: false })
    }

    pub fn is_open(&self) -> bool { !self.closed && self.connection.is_open() }
    pub fn history_count(&self) -> usize { self.history.len() }
    pub fn pending_client_count(&self) -> usize { self.pending_client.len() }

    pub fn close(&mut self) {
        self.closed = true; self.connection.close();
        self.history.clear(); self.working = Bucket::default(); self.pending_client.clear(); self.submitted_client.clear();
        self.pending_server.clear();
        self.dedup = DedupWindow::default();
        if let Some((_, active, _)) = self.prepared.take() { active.store(false, Ordering::Release); }
    }
    fn require_open(&mut self) -> Result<(), BatchError> {
        if !self.is_open() { self.close(); Err(BatchError::Closed) } else { Ok(()) }
    }
    fn fail<T>(&mut self, error: BatchError) -> Result<T, BatchError> { self.close(); Err(error) }
    fn clock(&mut self, now_ms: u64) -> Result<(), BatchError> {
        self.require_open()?;
        if now_ms < self.clock_ms { return self.fail(BatchError::Clock); }
        self.clock_ms = now_ms;
        Ok(())
    }
    fn ensure_retention(&mut self, now_ms: u64) -> Result<(), BatchError> {
        if self.history.values().any(|epoch| now_ms - epoch.created_at_ms >= BATCH_RETENTION_MS) {
            return self.fail(BatchError::Retention);
        }
        Ok(())
    }
    fn totals(&self) -> (usize, usize, usize) {
        let mut requests = BTreeMap::new(); let mut results = BTreeMap::new();
        for bucket in std::iter::once(&self.working).chain(self.history.values().map(|e| &e.bucket)) {
            requests.extend(bucket.requests.iter().map(|(&s, r)| (s, r.bytes)));
            results.extend(bucket.results.iter().map(|(&s, r)| (s, r.bytes())));
        }
        (requests.len(), results.len(), requests.values().sum::<usize>() + results.values().sum::<usize>()
            + self.pending_server.values().sum::<usize>())
    }

    /// Only an originally prepared hosting call may enter this scheduler. No
    /// sequence reallocation, arbitrary account override or implicit submission.
    pub fn queue_hosting(&mut self, call: &PreparedHostingCall, metadata: &[super::rpc_stream::RpcProperty<'_>])
        -> Result<(), BatchError> {
        self.require_open()?;
        if !call.matches(&self.owner, &self.connection) { return Err(BatchError::Scope); }
        if call.sequence <= self.last_client_sequence { return Err(BatchError::Epoch); }
        if self.prepared.is_some() { return Err(BatchError::Backpressure); }
        let request_shape = call.as_batch_request(metadata);
        let size = RequestData::measure(&request_shape)?;
        let (count, _, bytes) = self.totals();
        if self.pending_client.len() >= MAX_MAP_ENTRIES || count >= MAX_MAP_ENTRIES
            || bytes.checked_add(size).is_none_or(|n| n > MAX_RETAINED_BYTES) {
            return Err(BatchError::Backpressure);
        }
        let request = RequestData::capture(&request_shape)?;
        self.last_client_sequence = call.sequence;
        self.pending_client.insert(call.sequence);
        self.working.requests.insert(call.sequence, Arc::new(request));
        Ok(())
    }

    /// Cancel local interest and all retry copies, but never reuse its sequence.
    /// Tracker retirement is separate; ACK does not mean business completion.
    pub fn retire_client_request(&mut self, sequence: i32) {
        if self.prepared.is_some() { self.close(); return; }
        self.pending_client.remove(&sequence); self.submitted_client.remove(&sequence);
        self.working.requests.remove(&sequence);
        for epoch in self.history.values_mut() { epoch.bucket.requests.remove(&sequence); }
    }

    /// Build one immutable snapshot. commit_submission must occur only after the
    /// owning ARC encoder accepts it. Failure closes scope instead of silently
    /// treating an encoded/queued request as successful or resetting history.
    pub fn prepare(&mut self, now_ms: u64) -> Result<ScheduledBatch, BatchError> {
        self.clock(now_ms)?; self.ensure_retention(now_ms)?;
        if self.prepared.is_some() { return Err(BatchError::Backpressure); }
        let Some(serial) = self.next_serial else { return self.fail(BatchError::Epoch); };
        let mut bucket = self.working.clone();
        let has_work = !bucket.requests.is_empty() || !bucket.results.is_empty();
        let next = if has_work {
            if self.history.len() >= MAX_BATCH_EPOCHS { return self.fail(BatchError::Retention); }
            let Some(next) = self.next_batch.checked_add(1).filter(|&n| n <= i32::MAX as u32)
                else { return self.fail(BatchError::Epoch); };
            next
        } else { self.next_batch };
        let mut mask = u32::from(has_work);
        for (&epoch, retained) in &self.history {
            let bit = next.checked_sub(epoch + 1).ok_or(BatchError::Epoch)?;
            if bit >= 32 { return self.fail(BatchError::Retention); }
            mask |= 1u32 << bit;
            bucket.requests.extend(retained.bucket.requests.iter().map(|(&s, r)| (s, r.clone())));
            bucket.results.extend(retained.bucket.results.iter().map(|(&s, r)| (s, r.clone())));
        }
        let active = Arc::new(AtomicBool::new(true));
        let encoded = Arc::new(AtomicBool::new(false));
        self.prepared = Some((serial, active.clone(), encoded.clone()));
        Ok(ScheduledBatch { owner: self.owner.clone(), connection: self.connection.clone(), scheduler: self.identity.clone(), active, encoded,
            serial, words: BatchWords { incarnation: self.local_incarnation, next_batch: next, batches: mask,
                ack_incarnation: self.peer_incarnation.unwrap_or(0), ack_next_batch: self.peer_next, ack_batches: self.peer_batches }, bucket })
    }

    /// Abandon only an unsent/unencoded snapshot without forgetting work.
    /// Cancelling a snapshot whose bytes may have escaped closes the scope; it
    /// cannot pretend an unknown remote outcome was never submitted.
    pub fn cancel_prepared(&mut self) {
        if let Some((_, active, encoded)) = self.prepared.take() {
            active.store(false, Ordering::Release);
            if encoded.load(Ordering::Acquire) { self.close(); }
        }
    }

    pub fn commit_submission(&mut self, batch: ScheduledBatch, now_ms: u64) -> Result<(), BatchError> {
        self.clock(now_ms)?; self.ensure_retention(now_ms)?;
        if !batch.matches(&self.owner, &self.connection) || !Arc::ptr_eq(&self.identity, &batch.scheduler)
            || !batch.is_encoded()
            || self.prepared.as_ref().map(|(serial, _, _)| *serial) != Some(batch.serial) { return Err(BatchError::Scope); }
        if batch.words.next_batch != self.next_batch {
            if batch.words.next_batch != self.next_batch + 1 { return self.fail(BatchError::Epoch); }
            let bucket = std::mem::take(&mut self.working);
            self.submitted_client.extend(bucket.requests.keys().copied());
            self.history.insert(self.next_batch, Epoch { created_at_ms: now_ms, bucket });
            self.next_batch = batch.words.next_batch;
        }
        batch.active.store(false, Ordering::Release);
        self.prepared = None;
        self.next_serial = batch.serial.checked_add(1);
        Ok(())
    }

    /// Caller supplies only the already authenticated ARC reply carrying the
    /// captured scheduler identity. State is validated before any ACK/dedup
    /// mutation. ACK removes transport retention, never grants hosting/channels.
    pub fn on_reply(&mut self, batch: &OwnedBatchReply<'_>, now_ms: u64)
        -> Result<Option<BatchDelivery>, BatchError> {
        self.require_open()?;
        if !batch.matches(&self.owner, &self.connection) || !batch.matches_scheduler(&self.identity) {
            return Ok(None);
        }
        self.clock(now_ms)?;
        if self.prepared.is_some() { return Err(BatchError::Backpressure); }
        let reply = batch.reply();
        if !reply.success || !reply.footer.reason.is_empty() { return self.fail(BatchError::Epoch); }
        let words = BatchWords::from_fields(reply.payload.fields);
        if !valid_bitmap(words.next_batch, words.batches) || !valid_bitmap(words.ack_next_batch, words.ack_batches)
            || words.incarnation == 0 || words.next_batch > i32::MAX as u32
            || (words.ack_incarnation != 0 && words.ack_incarnation != self.local_incarnation)
            || (words.ack_incarnation == 0 && (words.ack_next_batch != 0 || words.ack_batches != 0))
            || words.ack_next_batch > self.next_batch {
            return self.fail(BatchError::Epoch);
        }
        if self.peer_incarnation.is_some_and(|epoch| epoch != words.incarnation)
            || (words.next_batch < self.peer_next && self.peer_next - words.next_batch >= 32) {
            return self.fail(BatchError::Epoch);
        }
        if reply.payload.requests.keys().chain(reply.payload.results.keys()).any(|&sequence| sequence <= 0)
            || ((!reply.payload.requests.is_empty() || !reply.payload.results.is_empty()) && words.batches == 0) {
            return self.fail(BatchError::Epoch);
        }
        let (_, _, retained_bytes) = self.totals();
        let incoming_bytes = match reply.payload.requests.values().try_fold(0usize, |sum, r| {
            let bytes = RequestData::measure(r)?;
            sum.checked_add(bytes).ok_or(CodecError::TooLarge)
        }) {
            Ok(bytes) => bytes, Err(error) => return self.fail(error.into()),
        };
        if retained_bytes.checked_add(incoming_bytes).is_none_or(|n| n > MAX_RETAINED_BYTES) {
            return self.fail(BatchError::Backpressure);
        }
        let mut fresh = Vec::new();
        let mut dedup = self.dedup.clone();
        for (&sequence, request) in &reply.payload.requests {
            if dedup.insert(sequence as u32) {
                let captured = match RequestData::capture(request) {
                    Ok(request) => request, Err(error) => return self.fail(error.into()),
                };
                fresh.push((sequence, captured));
            }
        }
        if self.pending_server.len() + fresh.len() > MAX_MAP_ENTRIES
            || fresh.iter().any(|(sequence, _)| self.pending_server.contains_key(sequence)) {
            return self.fail(BatchError::Backpressure);
        }
        if words.ack_incarnation == self.local_incarnation {
            for bit in 0..32 {
                if words.ack_batches & (1u32 << bit) != 0 { self.history.remove(&(words.ack_next_batch - 1 - bit)); }
            }
        }
        self.ensure_retention(now_ms)?;
        self.peer_incarnation = Some(words.incarnation);
        merge_bitmap(&mut self.peer_next, &mut self.peer_batches, words.next_batch, words.batches);
        self.dedup = dedup;
        let mut requests = Vec::with_capacity(fresh.len());
        for (sequence, request) in fresh {
            self.pending_server.insert(sequence, request.bytes);
            requests.push(IncomingBatchRequest { request, ticket: ServerRequestTicket { owner: self.owner.clone(),
                connection: self.connection.clone(), scheduler: self.identity.clone(), peer_incarnation: words.incarnation, sequence } });
        }
        let mut results = BTreeMap::new();
        for (&sequence, result) in &reply.payload.results {
            if self.pending_client.contains(&sequence) && self.submitted_client.contains(&sequence) {
                results.insert(sequence, ResultData { code: result.result_code, stream: result.result_stream.to_vec() });
            }
        }
        // Commit original pending retirement only after all delivery payloads
        // have been copied/validated; no partially retired successful batch.
        for sequence in results.keys() {
            self.submitted_client.remove(sequence); self.pending_client.remove(sequence);
        }
        let matched_result_sequences = results.keys().copied().collect();
        // Native epoch history holds immutable original calls until ACK/retention;
        // even an AgentCall result does not itself change the six ACK words.
        Ok(Some(BatchDelivery { requests, matched_result_sequences, results,
            owner: self.owner.clone(), connection: self.connection.clone() }))
    }

    pub fn complete_server_request(&mut self, ticket: &ServerRequestTicket, code: u32, stream: &[u8])
        -> Result<bool, BatchError> {
        self.require_open()?;
        if self.prepared.is_some() { return Err(BatchError::Backpressure); }
        if !ticket.matches(&self.owner, &self.connection) || !Arc::ptr_eq(&self.identity, &ticket.scheduler)
            || self.peer_incarnation != Some(ticket.peer_incarnation) { return Err(BatchError::Scope); }
        let Some(&pending_bytes) = self.pending_server.get(&ticket.sequence) else { return Ok(false); };
        if stream.len() > MAX_FIELD_BYTES { return self.fail(BatchError::Backpressure); }
        let (_, count, total_bytes) = self.totals();
        let bytes = total_bytes - pending_bytes;
        if count >= MAX_MAP_ENTRIES || bytes.checked_add(12 + stream.len()).is_none_or(|n| n > MAX_RETAINED_BYTES) {
            return self.fail(BatchError::Backpressure);
        }
        self.pending_server.remove(&ticket.sequence);
        self.working.results.insert(ticket.sequence, Arc::new(ResultData { code, stream: stream.to_vec() }));
        Ok(true)
    }
}

impl Drop for BatchScheduler {
    fn drop(&mut self) { self.close(); }
}

fn valid_bitmap(next: u32, bits: u32) -> bool { next >= 32 || bits >> next == 0 }

fn merge_bitmap(next: &mut u32, bits: &mut u32, incoming_next: u32, incoming_bits: u32) {
    if incoming_next >= *next {
        let delta = incoming_next - *next;
        *bits = if delta >= 32 { incoming_bits } else { (*bits << delta) | incoming_bits };
        *next = incoming_next;
    } else {
        let delta = *next - incoming_next;
        if delta < 32 { *bits |= incoming_bits << delta; }
    }
}

#[cfg(test)]
#[path = "batch_scheduler_tests.rs"]
mod tests;
