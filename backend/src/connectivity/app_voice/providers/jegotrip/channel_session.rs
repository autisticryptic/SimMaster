//! One original non-resumable Channel + directional NetStream record session.
//! A linear original-port claim and immutable negotiated PeerTarget are required;
//! pure Channel bytes/legacy XOR never confer SDK login or hosting authority.
use super::{native_routing::Routing, netstream_records::{RecordReceiver, RecordSecurity, RecordSender, escape_channel, unescape_channel},
    object_reference::ReferenceEndpoint, reliable_channel::{ChannelCore, ChannelError}, router_peer::{PeerBinding, PeerTarget},
    router_ports::{ChannelLease, LogicalAddress, PortClock, PortDelivery, PortError, PortLease, PortTransmitPermit},
    rpc_pending::RpcConnectionScope, session::AttemptOwner, transport_packets::{TransportBytes, TransportError}};
use std::{fmt, sync::{Arc, Weak, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionError { Scope, Configuration, Port(PortError), Channel(ChannelError), Record(TransportError) }
impl From<PortError> for SessionError { fn from(error: PortError) -> Self { Self::Port(error) } }
impl From<ChannelError> for SessionError { fn from(error: ChannelError) -> Self { Self::Channel(error) } }
impl From<TransportError> for SessionError { fn from(error: TransportError) -> Self { Self::Record(error) } }

struct Brand {
    owner: AttemptOwner, scope: RpcConnectionScope, live: Weak<AtomicBool>, peer: PeerTarget, permit: PortTransmitPermit,
}
impl Brand {
    fn for_session(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, session: &ChannelSession) -> bool {
        if &self.owner != owner || !self.scope.matches(scope) || !self.scope.is_open() || !session.original(owner, scope)
            || !self.peer.same_selection(session.peer.target()) || self.permit.address_for(owner, scope) != Some(session.local) { return false; }
        self.live.upgrade().is_some_and(|live| Arc::ptr_eq(&live, &session.live) && live.load(Ordering::Acquire))
    }
}
/// Still Common RECORDS, not decoded/accepted RPC, SDK authentication or a
///business ACK. A cancelled original session revokes already queued groups.
pub(super) struct ScopedRecords { brand: Brand, records: Vec<TransportBytes>, received: Instant }
impl ScopedRecords {
    pub fn received_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, session: &ChannelSession) -> Option<Instant> {
        self.brand.for_session(owner, scope, session).then_some(self.received)
    }
    pub fn records_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, session: &ChannelSession) -> Option<&[TransportBytes]> {
        self.brand.for_session(owner, scope, session).then_some(self.records.as_slice())
    }
}
impl fmt::Debug for ScopedRecords { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ScopedRecords(<redacted>)") } }
pub(super) struct PreparedChannel { brand: Brand, wire: TransportBytes, hard_end: Option<Instant>, prepared_at: Instant }
impl PreparedChannel {
    /// Inspection only; production physical IO uses for_io so the original
    ///hard expiration accompanies the bytes across every await.
    #[cfg(test)]
    pub fn bytes_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, session: &ChannelSession) -> Option<&[u8]> {
        self.brand.for_session(owner, scope, session).then_some(self.wire.expose_to_transport())
    }
    pub fn for_io(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, session: &ChannelSession,
        now: Instant, caller_end: Instant) -> Result<Option<(&[u8], Instant)>, SessionError> {
        if !self.brand.for_session(owner, scope, session) { return Ok(None); }
        if now < self.prepared_at { return Err(SessionError::Channel(ChannelError::ClockRollback)); }
        let end = self.hard_end.map_or(caller_end, |hard| hard.min(caller_end));
        if now >= end { return Err(SessionError::Channel(ChannelError::Timeout)); }
        Ok(Some((self.wire.expose_to_transport(), end)))
    }
}
impl fmt::Debug for PreparedChannel { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PreparedChannel(<redacted>)") } }

pub(super) struct ChannelSession {
    owner: AttemptOwner, scope: RpcConnectionScope, clock: PortClock, claim: ChannelLease,
    peer: PeerBinding, local: LogicalAddress, remote: LogicalAddress, live: Arc<AtomicBool>,
    core: ChannelCore, sender: RecordSender, receiver: RecordReceiver,
}
impl fmt::Debug for ChannelSession { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ChannelSession(<redacted>)") } }
impl Drop for ChannelSession { fn drop(&mut self) { self.close(); } }
impl ChannelSession {
    pub fn for_original(owner: AttemptOwner, scope: RpcConnectionScope, clock: PortClock, lease: &PortLease, peer: PeerBinding,
        endpoint: &ReferenceEndpoint, sequence_seed: u16, record_seed: u32, mtu: usize, startup_end: Instant,
        partial_timeout: Duration, now: Instant) -> Result<Self, SessionError> {
        if !clock.origin_matches(&owner, &scope) || !peer.target().matches_endpoint(endpoint)
            || peer.reply_for(&owner, &scope, &clock, peer.target()).is_none() { return Err(SessionError::Scope); }
        let (network, node, port) = peer.target().ids_for(&owner, &scope, &clock).ok_or(SessionError::Scope)?;
        let local = lease.address_for_clock(&owner, &scope, &clock).ok_or(SessionError::Scope)?;
        let mode = match endpoint.scheme() { b"arc" => RecordSecurity::Plain, b"sarc" => RecordSecurity::SeededXor,
            _ => return Err(SessionError::Configuration) };
        let sender = RecordSender::new(mode, (mode == RecordSecurity::SeededXor).then_some(record_seed))?;
        let core = ChannelCore::new(sequence_seed, mtu, startup_end, partial_timeout, now)?;
        let claim = lease.claim_channel(&owner, &scope, &clock)?.ok_or(SessionError::Scope)?;
        Ok(Self { owner, scope, clock, claim, peer, local, remote: LogicalAddress { network, node, port },
            live: Arc::new(AtomicBool::new(true)), core, sender, receiver: RecordReceiver::new(mode) })
    }
    fn original(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() && self.live.load(Ordering::Acquire)
            && self.claim.address_for(owner, scope) == Some(self.local)
            && self.peer.target().ids_for(owner, scope, &self.clock) == Some((self.remote.network, self.remote.node, self.remote.port))
    }
    fn brand(&self) -> Result<Brand, SessionError> {
        Ok(Brand { owner: self.owner.clone(), scope: self.scope.clone(), live: Arc::downgrade(&self.live), peer: self.peer.target().clone(),
            permit: self.claim.permit(&self.owner, &self.scope, &self.clock).ok_or(SessionError::Scope)? })
    }
    pub fn ready_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool { self.original(owner, scope) && self.core.ready() }
    pub fn has_queued(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool { self.original(owner, scope) && self.core.has_queued() }
    pub fn peer(&self) -> &PeerBinding { &self.peer }
    pub fn lease(&self) -> &PortLease { self.claim.lease() }
    pub fn next_wake(&self) -> Option<Instant> { self.original(&self.owner, &self.scope).then(|| self.core.next_wake()).flatten() }
    pub fn bound_io_deadline(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, caller_end: Instant) -> Option<Instant> {
        self.original(owner, scope).then(|| self.core.hard_deadline().map_or(caller_end, |hard| hard.min(caller_end)))
    }
    pub fn close(&mut self) {
        self.live.store(false, Ordering::Release); self.scope.close(); self.core.close(); self.sender.close(); self.receiver.close(); self.claim.release();
        // The enclosing RouterPath owner still owes physical socket Drop/Join.
    }
    pub fn tick(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, now: Instant) -> Result<bool, SessionError> {
        if !self.original(owner, scope) { return Ok(false); }
        let result = self.core.tick(now).map_err(SessionError::from); if result.is_err() { self.close(); } result.map(|()| true)
    }
    pub fn path_delay(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, milliseconds: Option<u16>) {
        if self.original(owner, scope) { self.core.set_echoed_delay(milliseconds); }
    }
    pub fn queue_record(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, plain: &[u8], deadline: Instant, now: Instant)
        -> Result<bool, SessionError> {
        if !self.original(owner, scope) { return Ok(false); }
        let result = (|| {
            let mut sender = self.sender.clone(); let encoded = sender.encode(plain)?;
            self.core.queue_record(encoded.expose_to_transport(), deadline, now)?;
            self.sender = sender; Ok(true)
        })();
        if result.is_err() { self.close(); } result
    }
    pub fn next_packet(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, now: Instant) -> Result<Option<PreparedChannel>, SessionError> {
        if !self.original(owner, scope) { return Ok(None); }
        let result = (|| {
            let Some(packet) = self.core.next_packet(now)? else { return Ok(None); };
            Ok(Some(PreparedChannel { brand: self.brand()?, wire: escape_channel(packet.expose_to_transport())?,
                hard_end: self.core.hard_deadline(), prepared_at: now }))
        })();
        if result.is_err() { self.close(); } result
    }
    fn from_original_peer(&self, routing: &Routing) -> bool {
        // D4EA28 selects source forwarding words for mask4, otherwise the
        //restored source network/node. These logical words NEVER authorise
        //DNS, another socket, TLS roots or a replacement SDK/account identity.
        let source = routing.source;
        let (network, node) = if source.mask & 4 != 0 { (source.forward_network, source.forward_node) } else { (source.network, source.node) };
        (network, node, source.port) == (self.remote.network, self.remote.node, self.remote.port)
    }
    /// Stage a WHOLE original Router delivery group. Foreign peer/lease/claim
    ///inputs are ignored BEFORE parsing or clock mutation. One later malformed
    ///matched input drops all staged plaintext and closes only this session.
    pub fn on_deliveries(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, deliveries: &[PortDelivery], now: Instant)
        -> Result<Option<ScopedRecords>, SessionError> {
        if !self.original(owner, scope) { return Ok(None); }
        let result = (|| {
            let mut matched = false; let mut records = Vec::new(); let mut total = 0usize; let mut oldest = now;
            for delivery in deliveries {
                let Some((routing, wire, received)) = delivery.for_channel(owner, scope, &self.claim) else { continue; };
                if !self.from_original_peer(routing) { continue; }
                if received > now { return Err(SessionError::Channel(ChannelError::ClockRollback)); }
                matched = true; oldest = oldest.min(received);
                for encoded in self.core.on_packet_admitted(unescape_channel(wire)?, received, now)? {
                    let plain = self.receiver.decode(encoded.expose_to_transport())?;
                    total = total.checked_add(plain.expose_to_transport().len()).ok_or(SessionError::Configuration)?;
                    if records.len() >= 256 || total > 1024 * 1024 { return Err(SessionError::Configuration); }
                    records.push(plain);
                }
            }
            if !matched { return Ok(None); }
            Ok(Some(ScopedRecords { brand: self.brand()?, records, received: oldest }))
        })();
        if result.is_err() { self.close(); } result
    }
}

#[cfg(test)]
#[path = "channel_session_tests.rs"]
mod tests;
