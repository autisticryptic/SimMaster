//! Recovered native Router path primitives. These are NOT SDK authentication,
//! NetStream byte delivery, service provisioning, or a provider registration.
//! The existing strict ordered Router codec remains available unchanged.
use super::{router_packets::{RouterHeader, MAX_ROUTED_BODY_BYTES},
    object_reference::{ReferenceEndpoint, RouterHost, RouterLogicalHost},
    packet_stream::MAX_NETWORK_PACKET_BYTES, transport_packets::{TransportBytes, TransportError}};
use std::{collections::BTreeMap, fmt, time::{Duration, Instant}};
use zeroize::Zeroize;

const HEADER_BYTES: usize = 8;
const MAX_CACHED_HALVES: usize = 64; // local resource policy, not a remote limit
const MAX_CACHED_BYTES: usize = MAX_ROUTED_BODY_BYTES * 2;
const HALF_AGE: Duration = Duration::from_millis(10_000);

/// Native kinds0/1 carry routing/data, kind2 requests timing feedback, kind3
/// returns feedback. Neither kind2 nor3 is an ARC/login/message acknowledgement.
pub fn decode_path_header(bytes: &[u8]) -> Result<(RouterHeader, &[u8]), TransportError> {
    if bytes.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
    let h = bytes.get(..HEADER_BYTES).ok_or(TransportError::Truncated)?;
    if h[7] & 0x88 != 0 { return Err(TransportError::UnsupportedKind); }
    let header = RouterHeader { sequence: u16::from_be_bytes([h[0],h[1]]),
        word1: u16::from_be_bytes([h[2],h[3]]), word2: u16::from_be_bytes([h[4],h[5]]),
        byte6: h[6], packet_kind: h[7] & 7, fragment_part: (h[7] >> 4) & 7 };
    validate(&header)?;
    Ok((header, &bytes[HEADER_BYTES..]))
}
fn validate(header: &RouterHeader) -> Result<(), TransportError> {
    if header.packet_kind > 3 || !(1..=3).contains(&header.fragment_part)
        || header.packet_kind > 1 && header.fragment_part != 1 { return Err(TransportError::UnsupportedKind); }
    Ok(())
}
fn packet(header: RouterHeader, body: &[u8]) -> Result<TransportBytes, TransportError> {
    validate(&header)?;
    if body.len() > MAX_ROUTED_BODY_BYTES { return Err(TransportError::TooLarge); }
    let mut bytes = Vec::with_capacity(HEADER_BYTES + body.len());
    bytes.extend_from_slice(&header.sequence.to_be_bytes()); bytes.extend_from_slice(&header.word1.to_be_bytes());
    bytes.extend_from_slice(&header.word2.to_be_bytes()); bytes.push(header.byte6);
    bytes.push(header.packet_kind | header.fragment_part << 4); bytes.extend_from_slice(body);
    Ok(TransportBytes::from_bytes(bytes))
}

pub struct TimingProbe { pub peer_send_time: u16, pub peer_sequence: u16 }
impl fmt::Debug for TimingProbe { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("TimingProbe(<redacted>)") } }
/// A path-level probe can have no body or exactly two BE16 echoed peer words.
/// Alternate control payloads are not guessed from arbitrary trailing bytes.
pub fn encode_probe(header: RouterHeader, echo: Option<TimingProbe>) -> Result<TransportBytes, TransportError> {
    if header.packet_kind != 2 || header.fragment_part != 1 { return Err(TransportError::InvalidPacket); }
    let mut body = Vec::new(); if let Some(echo) = echo {
        body.extend_from_slice(&echo.peer_send_time.to_be_bytes()); body.extend_from_slice(&echo.peer_sequence.to_be_bytes());
    }
    packet(header, &body)
}
pub fn encode_feedback(header: RouterHeader) -> Result<TransportBytes, TransportError> {
    if header.packet_kind != 3 || header.fragment_part != 1 { return Err(TransportError::InvalidPacket); }
    packet(header, &[])
}
pub fn decode_probe(body: &[u8]) -> Result<Option<TimingProbe>, TransportError> {
    match body {
        [] => Ok(None),
        [a,b,c,d] => Ok(Some(TimingProbe { peer_send_time: u16::from_be_bytes([*a,*b]), peer_sequence: u16::from_be_bytes([*c,*d]) })),
        _ => Err(TransportError::InvalidPacket),
    }
}

/// Native u16 wrap is confined to this ORIGINAL path. Values are not reusable
/// ARC IDs, and caller must not transfer this state to a replacement path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveObservation { FirstOrReset, New, Duplicate }
pub struct SequenceBookkeeping { words: [u32; 64], head: usize, base: Option<u16> }
impl Default for SequenceBookkeeping { fn default() -> Self { Self { words: [0;64], head: 0, base: None } } }
impl fmt::Debug for SequenceBookkeeping { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("SequenceBookkeeping(<redacted>)") } }
impl SequenceBookkeeping {
    pub fn observe(&mut self, sequence: u16) -> ReceiveObservation {
        let mut reset = false;
        let mut distance = self.base.map(|base| sequence.wrapping_sub(base) as i16);
        if distance.is_none_or(|d| !(0..=2047).contains(&d)) {
            self.words.fill(0); self.head = 0; self.base = Some(sequence.wrapping_sub(512)); distance = Some(512); reset = true;
        }
        let mut distance = distance.unwrap_or(512) as usize;
        if distance > 1023 {
            let advance = (distance - 1024) / 32 + 1;
            for _ in 0..advance { self.words[self.head] = 0; self.head = (self.head + 1) & 63; }
            self.base = self.base.map(|base| base.wrapping_add((advance * 32) as u16)); distance -= advance * 32;
        }
        let index = (self.head + distance / 32) & 63; let bit = 1u32 << (distance & 31);
        if self.words[index] & bit != 0 { return ReceiveObservation::Duplicate; }
        self.words[index] |= bit;
        if reset { ReceiveObservation::FirstOrReset } else { ReceiveObservation::New }
    }
}

struct Half { header: RouterHeader, bytes: Vec<u8>, received: Instant }
impl Drop for Half { fn drop(&mut self) { self.bytes.zeroize(); } }
pub struct PathMessage { pub first_header: RouterHeader, pub last_header: Option<RouterHeader>, pub body: TransportBytes }
impl fmt::Debug for PathMessage { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PathMessage(<redacted>)") } }
pub enum PathInput {
    Data { observation: ReceiveObservation, message: Option<PathMessage> },
    Probe { header: RouterHeader, echo: Option<TimingProbe> },
    Feedback { header: RouterHeader },
}
impl fmt::Debug for PathInput { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PathInput(<redacted>)") } }

/// Bidirectional half pairing with the recovered sequence-reset/window/age
/// cleanup. Path RTT/loss/keepalive feedback is a separate state machine. The bitmap
/// is BOOKKEEPING only: duplicate full packets are still returned as native
/// RouterPathI does. Reliable NetStream, not this Router path, decides delivery.
pub struct PathReceiver {
    bookkeeping: SequenceBookkeeping, halves: BTreeMap<u16, Half>, cached_bytes: usize,
    highest: Option<u16>, last_clock: Option<Instant>, closed: bool,
}
impl Default for PathReceiver { fn default() -> Self { Self { bookkeeping: SequenceBookkeeping::default(), halves: BTreeMap::new(),
    cached_bytes: 0, highest: None, last_clock: None, closed: false } } }
impl fmt::Debug for PathReceiver { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("PathReceiver")
    .field("closed", &self.closed).field("cached_halves", &self.halves.len()).field("data", &"<redacted>").finish() } }
impl PathReceiver {
    pub fn has_partial(&self) -> bool { !self.halves.is_empty() }
    pub fn close(&mut self) { self.halves.clear(); self.cached_bytes = 0; self.closed = true; }
    pub fn on_packet(&mut self, bytes: &[u8], now: Instant) -> Result<PathInput, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(bytes, now); if result.is_err() { self.close(); } result
    }
    fn consume(&mut self, bytes: &[u8], now: Instant) -> Result<PathInput, TransportError> {
        if self.last_clock.is_some_and(|last| now < last) { return Err(TransportError::InvalidPacket); }
        self.last_clock = Some(now);
        let (header, body) = decode_path_header(bytes)?;
        let observation = self.bookkeeping.observe(header.sequence);
        if observation == ReceiveObservation::FirstOrReset { self.halves.clear(); self.cached_bytes = 0; self.highest = Some(header.sequence); }
        if self.highest.is_none_or(|highest| header.sequence.wrapping_sub(highest) as i16 > 0) { self.highest = Some(header.sequence); }
        // Expire on every admitted input as a conservative local retention
        // rule; native pairing also performs this check before consuming halves.
        let highest = self.highest.unwrap_or(header.sequence);
        let cached_bytes = &mut self.cached_bytes;
        self.halves.retain(|&sequence, half| {
            let keep = now.duration_since(half.received) < HALF_AGE && (highest.wrapping_sub(sequence) as i16) < 1000;
            if !keep { *cached_bytes -= half.bytes.len(); } keep
        });
        match header.packet_kind {
            2 => return Ok(PathInput::Probe { header, echo: decode_probe(body)? }),
            3 if body.is_empty() => return Ok(PathInput::Feedback { header }),
            3 => return Err(TransportError::InvalidPacket),
            _ => {},
        }
        if body.is_empty() { return Err(TransportError::Truncated); }
        if header.fragment_part == 1 {
            return Ok(PathInput::Data { observation, message: Some(PathMessage { first_header: header, last_header: None,
                body: TransportBytes::from_bytes(body.to_vec()) }) });
        }
        let peer = if header.fragment_part == 2 { header.sequence.wrapping_add(1) } else { header.sequence.wrapping_sub(1) };
        if let Some(half) = self.halves.get(&peer) {
            if half.header.fragment_part + header.fragment_part != 5 || half.header.packet_kind != header.packet_kind {
                return Err(TransportError::InvalidFragment);
            }
            if body.len() > MAX_ROUTED_BODY_BYTES - half.bytes.len() { return Err(TransportError::TooLarge); }
            let half = self.halves.remove(&peer).ok_or(TransportError::InvalidFragment)?;
            self.cached_bytes -= half.bytes.len();
            let mut joined = Vec::with_capacity(half.bytes.len() + body.len());
            let (first_header, last_header) = if header.fragment_part == 2 {
                joined.extend_from_slice(body); joined.extend_from_slice(&half.bytes); (header, half.header)
            } else {
                joined.extend_from_slice(&half.bytes); joined.extend_from_slice(body); (half.header, header)
            };
            return Ok(PathInput::Data { observation, message: Some(PathMessage { first_header, last_header: Some(last_header),
                body: TransportBytes::from_bytes(joined) }) });
        }
        if let Some(half) = self.halves.get(&header.sequence) {
            if half.header.packet_kind != header.packet_kind || half.header.fragment_part != header.fragment_part || half.bytes != body {
                return Err(TransportError::InvalidFragment);
            }
            return Ok(PathInput::Data { observation, message: None });
        }
        if self.halves.len() >= MAX_CACHED_HALVES || body.len() > MAX_CACHED_BYTES - self.cached_bytes { return Err(TransportError::TooLarge); }
        self.cached_bytes += body.len(); self.halves.insert(header.sequence, Half { header, bytes: body.to_vec(), received: now });
        Ok(PathInput::Data { observation, message: None })
    }
    pub fn finish_eof(&mut self) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let partial = self.has_partial(); self.close(); if partial { Err(TransportError::Truncated) } else { Ok(()) }
    }
}

/// Original Router-agent connection control over the separately verified
/// endpoint packet boundary. This is NOT an SDK login or incoming grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouterConnectError { Configuration, Pending, Timeout, ClockRollback, Rejected, Protocol }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouterConnectPhase { Initial, Pending, Connected, Closed }

pub struct PreparedRouterConnect {
    owner: super::session::AttemptOwner,
    scope: super::rpc_pending::RpcConnectionScope,
    packets: Vec<TransportBytes>,
}
impl PreparedRouterConnect {
    pub fn packets_for(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<&[TransportBytes]> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(&self.packets) } else { None }
    }
}
impl fmt::Debug for PreparedRouterConnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PreparedRouterConnect(<redacted>)") }
}
pub struct RouterAssignment {
    owner: super::session::AttemptOwner,
    scope: super::rpc_pending::RpcConnectionScope,
    router: u32,
    node: u32,
}
impl RouterAssignment {
    /// Logical native IDs, not an OS address/port or SDK/hosting permission.
    pub fn ids_for(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<(u32, u32)> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some((self.router, self.node)) } else { None }
    }
}
impl fmt::Debug for RouterAssignment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterAssignment(<redacted>)") }
}

/// Original endpoint/query capture, not a mutable current-account lookup.
struct PendingResolve { call_id: i32, scheme: Vec<u8>, query: Vec<u8>, port: u16, deadline: Instant }
impl Drop for PendingResolve { fn drop(&mut self) { self.scheme.zeroize(); self.query.zeroize(); } }

pub struct PreparedRouterResolve {
    owner: super::session::AttemptOwner, scope: super::rpc_pending::RpcConnectionScope,
    call_id: i32, packets: Vec<TransportBytes>,
}
impl PreparedRouterResolve {
    pub fn call_id(&self) -> i32 { self.call_id }
    pub fn packets_for(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<&[TransportBytes]> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(&self.packets) } else { None }
    }
}
impl fmt::Debug for PreparedRouterResolve {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PreparedRouterResolve(<redacted>)") }
}

/// Only a complete, matched clientResolveHosts reply can create this object.
/// The original endpoint's scheme/host/port and original live scope all remain
/// required. Returned IDs cannot authorize an OS socket, SDK login or receiver.
pub struct RouterResolution {
    owner: super::session::AttemptOwner, scope: super::rpc_pending::RpcConnectionScope,
    call_id: i32, scheme: Vec<u8>, query: Vec<u8>, port: u16, hosts: Vec<RouterLogicalHost>,
}
impl RouterResolution {
    pub fn call_id(&self) -> i32 { self.call_id }
    pub fn hosts_for(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope,
        endpoint: &ReferenceEndpoint) -> Option<&[RouterLogicalHost]> {
        if &self.owner != owner || !self.scope.matches(scope) || !self.scope.is_open()
            || endpoint.scheme() != self.scheme.as_slice() || endpoint.port() != self.port { return None; }
        match endpoint.router_host().ok()? {
            RouterHost::Query(query) if query == self.query.as_slice() => Some(&self.hosts), _ => None,
        }
    }
}
impl Drop for RouterResolution { fn drop(&mut self) { self.scheme.zeroize(); self.query.zeroize(); self.hosts.clear(); } }
impl fmt::Debug for RouterResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterResolution(<redacted>)") }
}

/// Endpoint-TLS/path-validated data still awaiting logical port/Channel checks.
/// Retaining it prevents a control reply in the same TLS group from discarding
/// application traffic. This boundary deliberately grants no Common plaintext.
pub struct VerifiedRouterData {
    owner: super::session::AttemptOwner, scope: super::rpc_pending::RpcConnectionScope, message: PathMessage,
}
impl VerifiedRouterData {
    pub fn message_for(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<&PathMessage> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(&self.message) } else { None }
    }
}
impl fmt::Debug for VerifiedRouterData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("VerifiedRouterData(<redacted>)") }
}

pub struct RouterConnectTracker {
    owner: super::session::AttemptOwner,
    scope: super::rpc_pending::RpcConnectionScope,
    router_id: i32,
    nonce: u32,
    access: Vec<(&'static [u8], zeroize::Zeroizing<Vec<u8>>)>,
    next_control_id: Option<i32>,
    pending_resolve: Option<PendingResolve>,
    resolution: Option<RouterResolution>,
    data: Vec<VerifiedRouterData>,
    data_bytes: usize,
    mtu: usize,
    sequence: u16,
    deadline: Instant,
    last_now: Instant,
    phase: RouterConnectPhase,
    receiver: PathReceiver,
    common: super::transport_packets::CallAssembler,
}
impl fmt::Debug for RouterConnectTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterConnectTracker(<redacted>)") }
}
impl RouterConnectTracker {
    /// Bind before any IO. ri is captured from the original selected endpoint;
    /// accessKey/accessAuth must be a proven shared-context pair, never a phone
    /// token/SipPwd substitute. Fresh native node is zero. No default host/root,
    /// reverse address or original-owner replacement is inferred here.
    pub(super) fn for_original(owner: super::session::AttemptOwner, scope: super::rpc_pending::RpcConnectionScope,
        router_id: i32, nonce: u32, access: &[super::rpc_stream::RpcProperty<'_>], mtu: usize,
        sequence: u16, timeout: Duration, now: Instant) -> Result<Self, RouterConnectError> {
        if !scope.is_open() || router_id < -999 || router_id == 0 || nonce == 0 || !(16..=MAX_NETWORK_PACKET_BYTES).contains(&mtu)
            || timeout.is_zero() || timeout > Duration::from_secs(30) { return Err(RouterConnectError::Configuration); }
        if !access.is_empty() && (access.len() != 2 || ![b"accessKey".as_slice(), b"accessAuth".as_slice()].iter()
            .all(|key| access.iter().filter(|p| p.key == *key && !p.value.is_empty() && p.value.len() <= 4096).count() == 1)) {
            return Err(RouterConnectError::Configuration);
        }
        let access = access.iter().map(|p| {
            let key = if p.key == b"accessKey" { b"accessKey".as_slice() } else { b"accessAuth".as_slice() };
            (key, zeroize::Zeroizing::new(p.value.to_vec()))
        }).collect();
        Ok(Self { owner, scope, router_id, nonce, access, mtu, sequence,
            next_control_id: Some(2), pending_resolve: None, resolution: None, data: Vec::new(), data_bytes: 0,
            deadline: now.checked_add(timeout).ok_or(RouterConnectError::Configuration)?, last_now: now,
            phase: RouterConnectPhase::Initial, receiver: PathReceiver::default(), common: super::transport_packets::CallAssembler::default() })
    }
    pub fn phase(&self) -> RouterConnectPhase { if self.scope.is_open() { self.phase } else { RouterConnectPhase::Closed } }
    pub fn close(&mut self) {
        self.phase = RouterConnectPhase::Closed; self.scope.close(); self.receiver.close(); self.common.close(); self.access.clear();
        self.pending_resolve = None; self.resolution = None; self.data.clear(); self.data_bytes = 0;
    }
    fn accepts(&self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    fn check_time(&mut self, now: Instant) -> Result<(), RouterConnectError> {
        let error = if now < self.last_now { Some(RouterConnectError::ClockRollback) }
            else if (self.phase != RouterConnectPhase::Connected && now >= self.deadline)
                || self.pending_resolve.as_ref().is_some_and(|pending| now >= pending.deadline) { Some(RouterConnectError::Timeout) } else { None };
        if let Some(error) = error { self.close(); return Err(error); }
        self.last_now = now; Ok(())
    }
    pub fn tick(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope,
        now: Instant) -> Result<bool, RouterConnectError> {
        if !self.accepts(owner, scope) { return Ok(false); }
        self.check_time(now)?; Ok(true)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.scope.is_open() { return None; }
        self.pending_resolve.as_ref().map(|pending| pending.deadline)
            .or_else(|| (self.phase != RouterConnectPhase::Connected).then_some(self.deadline))
    }
    pub fn prepare(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope,
        timing: super::packet_transport::RouterTiming, now: Instant) -> Result<Option<PreparedRouterConnect>, RouterConnectError> {
        if !self.accepts(owner, scope) { return Ok(None); }
        self.check_time(now)?;
        if self.phase != RouterConnectPhase::Initial { return Err(RouterConnectError::Pending); }
        use super::rpc_stream::Writer;
        let mut args = Writer::versioned().map_err(|_| RouterConnectError::Configuration)?;
        args.u32(0).map_err(|_| RouterConnectError::Configuration)?;
        args.u32(self.nonce).map_err(|_| RouterConnectError::Configuration)?;
        let method = if self.router_id > 0 {
            args.u32(self.router_id as u32).map_err(|_| RouterConnectError::Configuration)?;
            b"clientConnect1.RouterClient.Common".as_slice()
        } else { b"clientConnect3.RouterClient.Common".as_slice() };
        let args = args.finish();
        let packets = self.control_packets(1, method, args.expose_to_transport(), timing)?;
        self.phase = RouterConnectPhase::Pending;
        Ok(Some(PreparedRouterConnect { owner: self.owner.clone(), scope: self.scope.clone(), packets }))
    }
    fn control_packets(&mut self, call_id: i32, method: &[u8], arguments: &[u8],
        timing: super::packet_transport::RouterTiming) -> Result<Vec<TransportBytes>, RouterConnectError> {
        use super::rpc_stream::RpcProperty;
        let mut metadata = vec![RpcProperty { key: b"id", value: b"Router" }, RpcProperty { key: b"arcVer", value: b"16777216" }];
        metadata.extend(self.access.iter().map(|(key, value)| RpcProperty { key, value }));
        let request = super::rpc_envelope::encode_request(call_id, &metadata, method, arguments).map_err(|_| RouterConnectError::Configuration)?;
        let common = super::transport_packets::encode_call_packets(super::transport_packets::CallKind::Request,
            request.expose_to_transport(), super::transport_packets::TLS_FRAGMENT_THRESHOLD).map_err(|_| RouterConnectError::Configuration)?;
        let mut packets = Vec::new(); let mut sequence = self.sequence;
        for item in common {
            // Original Router-agent controls use the recovered fresh zero route,
            // not the logical destination of an AccountEntry/Channel packet.
            let route = super::router_packets::RouterRouting { tag: 0, flags: 0, path_words: &[],
                address1: super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[0, 0] },
                address2: super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[0, 0] } };
            let body = super::router_packets::encode_routed_body(&route, item.expose_to_transport()).map_err(|_| RouterConnectError::Configuration)?;
            let mut framed = super::router_packets::encode_router_packets(RouterHeader { sequence, word1: timing.word1,
                word2: timing.word2, byte6: timing.byte6, packet_kind: 1, fragment_part: 1 }, body.expose_to_transport(), self.mtu)
                .map_err(|_| RouterConnectError::Configuration)?;
            sequence = sequence.wrapping_add(framed.len() as u16); packets.append(&mut framed);
        }
        self.sequence = sequence; Ok(packets)
    }
    /// E33008: version1/0 followed by the query WITHOUT its initial arc@.
    /// Only one outstanding resolution per original path. The caller supplies
    /// an absolute deadline clamped to its original overall attempt budget;
    /// packet traffic and duplicate submissions cannot renew this deadline.
    pub fn prepare_resolve(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope,
        endpoint: &ReferenceEndpoint, timing: super::packet_transport::RouterTiming, deadline: Instant, now: Instant)
        -> Result<Option<PreparedRouterResolve>, RouterConnectError> {
        if !self.accepts(owner, scope) { return Ok(None); }
        self.check_time(now)?;
        if self.phase != RouterConnectPhase::Connected || self.pending_resolve.is_some() || self.resolution.is_some() {
            return Err(RouterConnectError::Pending);
        }
        if deadline.checked_duration_since(now).is_none_or(|remaining| remaining.is_zero() || remaining > Duration::from_secs(30)) {
            return Err(RouterConnectError::Configuration);
        }
        let RouterHost::Query(query) = endpoint.router_host().map_err(|_| RouterConnectError::Configuration)? else {
            return Err(RouterConnectError::Configuration);
        };
        let call_id = self.next_control_id.ok_or(RouterConnectError::Configuration)?;
        let mut args = super::rpc_stream::Writer::versioned().map_err(|_| RouterConnectError::Configuration)?;
        args.bytes(query).map_err(|_| RouterConnectError::Configuration)?; let args = args.finish();
        let packets = self.control_packets(call_id, b"clientResolveHosts.RouterClient.Common", args.expose_to_transport(), timing)?;
        self.next_control_id = call_id.checked_add(1);
        self.pending_resolve = Some(PendingResolve { call_id, scheme: endpoint.scheme().to_vec(), query: query.to_vec(), port: endpoint.port(), deadline });
        Ok(Some(PreparedRouterResolve { owner: self.owner.clone(), scope: self.scope.clone(), call_id, packets }))
    }
    pub fn take_resolution(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<RouterResolution> {
        if !self.accepts(owner, scope) { return None; } self.resolution.take()
    }
    pub fn take_data(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope) -> Option<Vec<VerifiedRouterData>> {
        if !self.accepts(owner, scope) { return None; }
        self.data_bytes = 0; Some(std::mem::take(&mut self.data))
    }
    pub fn on_packets(&mut self, owner: &super::session::AttemptOwner, scope: &super::rpc_pending::RpcConnectionScope,
        input: super::tls_engine::VerifiedEndpointPackets, now: Instant) -> Result<Option<RouterAssignment>, RouterConnectError> {
        if !self.accepts(owner, scope) { return Ok(None); }
        let Some(packets) = input.packets_for(owner, scope) else { return Ok(None); };
        self.check_time(now)?;
        let result = self.consume(packets, now);
        if result.is_err() { self.close(); }
        result
    }
    fn consume(&mut self, packets: &[TransportBytes], now: Instant) -> Result<Option<RouterAssignment>, RouterConnectError> {
        let mut assignment = None; let mut resolution: Option<RouterResolution> = None;
        for raw in packets {
            let path = self.receiver.on_packet(raw.expose_to_transport(), now).map_err(|_| RouterConnectError::Protocol)?;
            let PathInput::Data { message: Some(message), .. } = path else { continue; };
            if message.first_header.packet_kind == 0 {
                if self.phase != RouterConnectPhase::Connected && assignment.is_none() { return Err(RouterConnectError::Protocol); }
                super::router_packets::decode_routed_body(message.body.expose_to_transport()).map_err(|_| RouterConnectError::Protocol)?;
                let bytes = message.body.expose_to_transport().len();
                if self.data.len() >= 64 || bytes > MAX_CACHED_BYTES - self.data_bytes { return Err(RouterConnectError::Protocol); }
                self.data_bytes += bytes;
                self.data.push(VerifiedRouterData { owner: self.owner.clone(), scope: self.scope.clone(), message });
                continue;
            }
            if message.first_header.packet_kind != 1 { return Err(RouterConnectError::Protocol); }
            let route = super::router_packets::decode_routed_body(message.body.expose_to_transport()).map_err(|_| RouterConnectError::Protocol)?;
            let Some(common) = self.common.on_packet(route.payload).map_err(|_| RouterConnectError::Protocol)? else { continue; };
            if matches!(common.kind, super::transport_packets::CallKind::Ping | super::transport_packets::CallKind::Pong) { continue; }
            if common.kind != super::transport_packets::CallKind::Reply { return Err(RouterConnectError::Protocol); }
            let reply = super::rpc_envelope::decode_reply(common.payload.expose_to_transport()).map_err(|_| RouterConnectError::Protocol)?;
            if self.phase == RouterConnectPhase::Pending && reply.call_id == 1 && assignment.is_none() {
                if reply.result_code != 0 { return Err(RouterConnectError::Rejected); }
                let mut reader = super::rpc_stream::Reader::new(reply.result_stream).map_err(|_| RouterConnectError::Protocol)?;
                let accepted = reader.boolean().map_err(|_| RouterConnectError::Protocol)?;
                let router = if self.router_id > 0 { self.router_id as u32 } else { reader.u32().map_err(|_| RouterConnectError::Protocol)? };
                let node = reader.u32().map_err(|_| RouterConnectError::Protocol)?;
                let footer = super::rpc_stream::read_footer(&mut reader).map_err(|_| RouterConnectError::Protocol)?;
                reader.finish().map_err(|_| RouterConnectError::Protocol)?;
                if !accepted || !footer.reason.is_empty() || router == 0 || node == 0 { return Err(RouterConnectError::Rejected); }
                assignment = Some(RouterAssignment { owner: self.owner.clone(), scope: self.scope.clone(), router, node });
            } else if let Some(pending) = self.pending_resolve.as_ref().filter(|pending| pending.call_id == reply.call_id) {
                if reply.result_code != 0 { return Err(RouterConnectError::Rejected); }
                let mut reader = super::rpc_stream::Reader::new(reply.result_stream).map_err(|_| RouterConnectError::Protocol)?;
                let accepted = reader.boolean().map_err(|_| RouterConnectError::Protocol)?;
                let count = reader.u32().map_err(|_| RouterConnectError::Protocol)?;
                if count > 32 { return Err(RouterConnectError::Protocol); }
                let mut hosts = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    let host = reader.bytes().map_err(|_| RouterConnectError::Protocol)?;
                    hosts.push(RouterLogicalHost::parse(host).map_err(|_| RouterConnectError::Protocol)?);
                }
                let footer = super::rpc_stream::read_footer(&mut reader).map_err(|_| RouterConnectError::Protocol)?;
                reader.finish().map_err(|_| RouterConnectError::Protocol)?;
                if !accepted || hosts.is_empty() || !footer.reason.is_empty() { return Err(RouterConnectError::Rejected); }
                if let Some(prior) = &resolution {
                    // Native path duplicates still reach the RPC layer. Parse
                    // the entire duplicate too; only an identical result is inert.
                    if prior.hosts != hosts { return Err(RouterConnectError::Protocol); }
                } else {
                    resolution = Some(RouterResolution { owner: self.owner.clone(), scope: self.scope.clone(), call_id: pending.call_id,
                        scheme: pending.scheme.clone(), query: pending.query.clone(), port: pending.port, hosts });
                }
            }
        }
        // Publish only after the ENTIRE endpoint-TLS group has parsed. Later
        // malformed packets close the scope and invalidate all retained results.
        if assignment.is_some() { self.phase = RouterConnectPhase::Connected; }
        if let Some(resolution) = resolution { self.pending_resolve = None; self.resolution = Some(resolution); }
        Ok(assignment)
    }
}
impl Drop for RouterConnectTracker { fn drop(&mut self) { self.close(); } }

#[cfg(test)]
#[path = "native_router_tests.rs"]
mod tests;
