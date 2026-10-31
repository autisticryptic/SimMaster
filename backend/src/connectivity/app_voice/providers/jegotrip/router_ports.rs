//! Original-path logical listen ownership (D41C38/D5ECF8/D571C0/D43698).
//!
//! A reservation is local, not clientPortStates, SDK authentication or a remote
//! registration ACK. The native receive path D4EA28 refreshes host availability
//! only on received routed traffic, NOT on clientConnect alone. Physical socket
//! Drop and owning-task Join remain the RouterPath/actor owner's responsibility.
use super::{native_router::{RouterAssignment, VerifiedRouterData},
    native_routing::{decode_routing_body, Routing}, rpc_pending::RpcConnectionScope,
    session::AttemptOwner, transport_packets::TransportBytes};
use std::{collections::BTreeMap, fmt, sync::{Arc, Mutex, MutexGuard, Weak, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};

struct ClockState { next: AtomicU64, claimed: AtomicBool }
/// One event order per ORIGINAL tracker, shared only with its single port book.
/// This is local admission ordering, never a wire sequence, nonce or RPC ID.
#[derive(Clone)]
pub(super) struct PortClock { owner: AttemptOwner, scope: RpcConnectionScope, inner: Arc<ClockState> }
pub(super) struct PortOrder { inner: Arc<ClockState>, value: u64 }
impl PortClock {
    pub fn for_original(owner: AttemptOwner, scope: RpcConnectionScope) -> Self {
        Self { owner, scope, inner: Arc::new(ClockState { next: AtomicU64::new(0), claimed: AtomicBool::new(false) }) }
    }
    pub fn stamp(&self) -> Result<PortOrder, PortError> {
        if !self.scope.is_open() { return Err(PortError::Closed); }
        let previous = self.inner.next.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| old.checked_add(1))
            .map_err(|_| PortError::Exhausted)?;
        Ok(PortOrder { inner: self.inner.clone(), value: previous + 1 })
    }
    pub fn origin_matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    pub fn owns(&self, order: &PortOrder) -> bool { Arc::ptr_eq(&self.inner, &order.inner) }
    fn value_for(&self, order: &PortOrder) -> Option<u64> {
        self.owns(order).then_some(order.value)
    }
}

const MAX_PORTS: usize = 64; // bounded project policy, not native map capacity
const FIRST_DYNAMIC_PORT: u16 = 10000;
const MAX_WAIT: Duration = Duration::from_secs(30);
const HOST_FRESHNESS: Duration = Duration::from_millis(6000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PortError { Configuration, InUse, Exhausted, ClockRollback, HostChanged, Protocol, Closed }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListenProtocol { Arc, Varc }

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LogicalAddress { pub network: u16, pub node: u32, pub port: u16 }
impl fmt::Debug for LogicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LogicalAddress(<redacted>)") }
}

struct Entry {
    generation: u64, birth_order: u64, protocol: ListenProtocol, deadline: Option<Instant>,
    address: Option<LogicalAddress>, lease: Weak<LeaseInner>,
}
struct State {
    ports: BTreeMap<u16, Entry>, addresses: BTreeMap<LogicalAddress, u64>,
    local: Option<(u16, u32)>, host_seen: Option<Instant>,
    next_port: u16, next_generation: Option<u64>, last_input: u64, last_now: Instant, closed: bool,
}
impl State {
    fn close(&mut self) { self.closed = true; self.ports.clear(); self.addresses.clear(); self.local = None; self.host_seen = None; }
    fn release(&mut self, port: u16, generation: u64) -> bool {
        let Some(entry) = self.ports.get(&port).filter(|entry| entry.generation == generation) else { return false; };
        // Remove the ORIGINAL full address before releasing the port. A stale
        // callback/Drop must never delete the replacement allocation's index.
        if let Some(address) = entry.address {
            if self.addresses.get(&address) == Some(&generation) { self.addresses.remove(&address); }
        }
        self.ports.remove(&port); true
    }
    fn tick(&mut self, now: Instant) -> Result<(), PortError> {
        if self.closed { return Err(PortError::Closed); }
        if now < self.last_now { self.close(); return Err(PortError::ClockRollback); }
        self.last_now = now;
        let expired: Vec<_> = self.ports.iter().filter(|(_, entry)| entry.deadline.is_some_and(|end| now >= end))
            .map(|(&port, entry)| (port, entry.generation)).collect();
        for (port, generation) in expired { self.release(port, generation); }
        Ok(())
    }
    fn fresh_host(&self, now: Instant) -> Option<(u16, u32)> {
        self.host_seen.and_then(|seen| now.checked_duration_since(seen)).filter(|age| *age < HOST_FRESHNESS)?;
        self.local
    }
    fn receive_host(&mut self, ids: (u16, u32), received: Instant, now: Instant) -> Result<(), PortError> {
        if self.local.is_some_and(|old| old != ids) { self.close(); return Err(PortError::HostChanged); }
        self.local = Some(ids); self.host_seen = Some(self.host_seen.map_or(received, |old| old.max(received)));
        if self.fresh_host(now).is_none() { return Ok(()); }
        // The caller has already expired waiting entries against their original
        // absolute deadlines. All registrations commit under this one lock.
        if self.ports.iter().any(|(&port, entry)| entry.address.is_none()
            && self.addresses.contains_key(&LogicalAddress { network: ids.0, node: ids.1, port })) {
            self.close(); return Err(PortError::Protocol);
        }
        for (&port, entry) in &mut self.ports {
            if entry.address.is_some() { continue; }
            let address = LogicalAddress { network: ids.0, node: ids.1, port };
            self.addresses.insert(address, entry.generation); entry.address = Some(address); entry.deadline = None;
        }
        Ok(())
    }
}

struct LeaseInner {
    state: Weak<Mutex<State>>, owner: AttemptOwner, scope: RpcConnectionScope,
    port: u16, generation: u64, birth: PortOrder,
}
impl Drop for LeaseInner {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
            state.release(self.port, self.generation);
        }
    }
}
/// Same-protocol reuse shares the original listen. Last owner Drop releases it;
/// a queued delivery holds only a Weak reference, so it cannot keep it alive.
#[derive(Clone)]
pub(super) struct PortLease { inner: Arc<LeaseInner> }
impl fmt::Debug for PortLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PortLease(<redacted>)") }
}
impl PortLease {
    fn original(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.inner.owner == owner && self.inner.scope.matches(scope)
    }
    pub fn address_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<LogicalAddress> {
        if !self.original(owner, scope) || !self.inner.scope.is_open() { return None; }
        let state = self.inner.state.upgrade()?; let state = state.lock().ok()?;
        if state.closed { return None; }
        state.ports.get(&self.inner.port).filter(|entry| entry.generation == self.inner.generation)?.address
    }
    pub fn address_for_clock(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, clock: &PortClock) -> Option<LogicalAddress> {
        if !clock.origin_matches(owner, scope) || !clock.owns(&self.inner.birth) { return None; }
        self.address_for(owner, scope)
    }
    pub fn transmit_permit(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, clock: &PortClock) -> Option<PortTransmitPermit> {
        let address = self.address_for_clock(owner, scope, clock)?;
        Some(PortTransmitPermit { lease: Arc::downgrade(&self.inner), address })
    }
    pub fn release(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        if !self.original(owner, scope) { return false; }
        let Some(state) = self.inner.state.upgrade() else { return false; };
        let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
        state.release(self.inner.port, self.inner.generation)
    }
}

pub(super) struct PortTransmitPermit { lease: Weak<LeaseInner>, address: LogicalAddress }
impl PortTransmitPermit {
    pub fn address_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<LogicalAddress> {
        let lease = PortLease { inner: self.lease.upgrade()? };
        (lease.address_for(owner, scope)? == self.address).then_some(self.address)
    }
}
impl fmt::Debug for PortTransmitPermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PortTransmitPermit(<redacted>)") }
}

pub(super) struct PortDelivery {
    lease: Weak<LeaseInner>, routing: Routing, payload: TransportBytes,
}
impl fmt::Debug for PortDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PortDelivery(<redacted>)") }
}
impl PortDelivery {
    pub fn retained_bytes(&self) -> usize { self.payload.expose_to_transport().len() }
    /// Admission to one logical listen only. The payload is still unaccepted
    /// Channel/NetStream data, never VerifiedRpcPlaintext. In particular, the
    /// on-wire Router address carries NO local allocation generation; delayed
    /// wire traffic after reuse still needs the Channel incarnation check.
    pub fn for_lease(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, lease: &PortLease)
        -> Option<(&Routing, &[u8])> {
        let original = self.lease.upgrade()?;
        if !Arc::ptr_eq(&original, &lease.inner) || !lease.original(owner, scope) { return None; }
        let address = lease.address_for(owner, scope)?;
        if address != (LogicalAddress { network: self.routing.destination.network, node: self.routing.destination.node,
            port: self.routing.destination.port }) { return None; }
        Some((&self.routing, self.payload.expose_to_transport()))
    }
}

pub(super) struct RouterPorts { owner: AttemptOwner, scope: RpcConnectionScope, clock: PortClock, state: Arc<Mutex<State>> }
impl fmt::Debug for RouterPorts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterPorts(<redacted>)") }
}
impl RouterPorts {
    pub fn for_original(owner: AttemptOwner, scope: RpcConnectionScope, clock: PortClock, first_candidate: u16, now: Instant) -> Result<Self, PortError> {
        if !scope.is_open() || clock.owner != owner || !clock.scope.matches(&scope) { return Err(PortError::Closed); }
        if clock.inner.claimed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return Err(PortError::InUse); }
        Ok(Self { owner, scope, clock, state: Arc::new(Mutex::new(State { ports: BTreeMap::new(), addresses: BTreeMap::new(),
            local: None, host_seen: None, next_port: first_candidate.max(FIRST_DYNAMIC_PORT), next_generation: Some(1), last_input: 0, last_now: now, closed: false })) })
    }
    fn original(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>, PortError> {
        self.state.lock().map_err(|poison| { poison.into_inner().close(); self.scope.close(); PortError::Closed })
    }
    fn tick_state(&self, state: &mut State, now: Instant) -> Result<(), PortError> {
        let result = state.tick(now); if result.is_err() { self.scope.close(); } result
    }
    pub fn close(&self) {
        self.scope.close(); self.state.lock().unwrap_or_else(|poison| poison.into_inner()).close();
    }
    pub fn tick(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, now: Instant) -> Result<bool, PortError> {
        if !self.original(owner, scope) { return Ok(false); }
        let mut state = self.lock()?; self.tick_state(&mut state, now)?; Ok(true)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.scope.is_open() { return None; }
        self.state.lock().ok()?.ports.values().filter_map(|entry| entry.deadline).min()
    }
    pub fn needs_host_refresh(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, now: Instant) -> bool {
        self.original(owner, scope) && self.state.lock().ok().is_some_and(|state| !state.closed && state.fresh_host(now).is_none())
    }
    pub fn reserve(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, protocol: ListenProtocol,
        requested: Option<u16>, reuse: bool, deadline: Instant, now: Instant) -> Result<Option<PortLease>, PortError> {
        if !self.original(owner, scope) { return Ok(None); }
        let mut state = self.lock()?; self.tick_state(&mut state, now)?;
        if requested == Some(0) || deadline.checked_duration_since(now).is_none_or(|left| left.is_zero() || left > MAX_WAIT) {
            return Err(PortError::Configuration);
        }
        let port = if let Some(port) = requested {
            if let Some(entry) = state.ports.get(&port) {
                if reuse && entry.protocol == protocol {
                    if let Some(inner) = entry.lease.upgrade() { return Ok(Some(PortLease { inner })); }
                }
                return Err(PortError::InUse);
            }
            port
        } else {
            if state.ports.len() >= MAX_PORTS { return Err(PortError::Exhausted); }
            let mut selected = None;
            for _ in FIRST_DYNAMIC_PORT..=u16::MAX {
                let candidate = state.next_port;
                state.next_port = candidate.checked_add(1).unwrap_or(FIRST_DYNAMIC_PORT);
                if !state.ports.contains_key(&candidate) { selected = Some(candidate); break; }
            }
            selected.ok_or(PortError::Exhausted)?
        };
        if state.ports.len() >= MAX_PORTS { return Err(PortError::Exhausted); }
        let generation = state.next_generation.ok_or(PortError::Exhausted)?;
        let address = state.fresh_host(now).map(|(network, node)| LogicalAddress { network, node, port });
        if address.is_some_and(|address| state.addresses.contains_key(&address)) { return Err(PortError::InUse); }
        let birth = self.clock.stamp()?; let birth_order = birth.value;
        let inner = Arc::new(LeaseInner { state: Arc::downgrade(&self.state), owner: self.owner.clone(), scope: self.scope.clone(), port, generation, birth });
        state.next_generation = generation.checked_add(1);
        state.ports.insert(port, Entry { generation, birth_order, protocol, deadline: address.is_none().then_some(deadline), address, lease: Arc::downgrade(&inner) });
        if let Some(address) = address { state.addresses.insert(address, generation); }
        Ok(Some(PortLease { inner }))
    }
    /// Called only with the original tracker's complete, whole-group validated
    /// kind0 data capability and its real assignment. clientConnect by itself
    /// cannot call this method with a synthetic routing body.
    pub fn on_data(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, assignment: &RouterAssignment,
        data: &VerifiedRouterData, now: Instant) -> Result<Option<PortDelivery>, PortError> {
        if !self.original(owner, scope) { return Ok(None); }
        let Some((message, order, received)) = data.admission_for(owner, scope) else { return Ok(None); };
        if self.clock.value_for(order).is_none() { return Ok(None); }
        let Some((network, node)) = assignment.ids_for(owner, scope) else { return Ok(None); };
        let result = (|| {
            let network = u16::try_from(network).map_err(|_| PortError::Protocol)?;
            if network == 0 || node == 0 || node == u32::MAX || message.first_header.packet_kind != 0 { return Err(PortError::Protocol); }
            let decoded = decode_routing_body(message.body.expose_to_transport()).map_err(|_| PortError::Protocol)?;
            self.receive_routed((network, node), decoded.routing, decoded.payload, order, received, now)
        })();
        if result.is_err() { self.close(); } result
    }
    fn receive_routed(&self, ids: (u16, u32), mut routing: Routing, payload: &[u8], order: &PortOrder,
        received: Instant, now: Instant) -> Result<Option<PortDelivery>, PortError> {
        let Some(input_order) = self.clock.value_for(order) else { return Ok(None); };
        let mut state = self.lock()?;
        if input_order <= state.last_input { return Ok(None); }
        self.tick_state(&mut state, now)?;
        if received > now { state.close(); self.scope.close(); return Err(PortError::ClockRollback); }
        state.last_input = input_order;
        if let Err(error) = state.receive_host(ids, received, now) { self.scope.close(); return Err(error); }
        // D4EA28 always restores destination to this receiving RouterItem's
        // local IDs. An explicit foreign destination never selects another
        // account/table. Missing source network uses this network, NOT its node.
        routing.destination.mask |= 3; routing.destination.network = ids.0; routing.destination.node = ids.1;
        if routing.source.mask & 1 == 0 { routing.source.mask |= 1; routing.source.network = ids.0; }
        let address = LogicalAddress { network: ids.0, node: ids.1, port: routing.destination.port };
        if address.port == 0 { return Ok(None); } // Router controls, never a listen
        let Some(&generation) = state.addresses.get(&address) else { return Ok(None); };
        let Some(entry) = state.ports.get(&address.port).filter(|entry| entry.generation == generation) else { return Err(PortError::Protocol); };
        if entry.birth_order >= input_order || entry.lease.strong_count() == 0 { return Ok(None); }
        Ok(Some(PortDelivery { lease: entry.lease.clone(), routing, payload: TransportBytes::from_bytes(payload.to_vec()) }))
    }
}
impl Drop for RouterPorts { fn drop(&mut self) { self.close(); } }

#[cfg(test)]
#[path = "router_ports_tests.rs"]
mod tests;
