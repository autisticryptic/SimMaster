//! Reliable default (Basic header) Channel state, below NetStream records.
//! F54668/F55168: incarnation +0x4000 and non-data sequence gaps;
//! F562E0/F55018: original send-history ACKs; F53CEC: preceding-DATA chain.
//! This private codec owns NO socket and grants NO SDK/business authentication.
//! Its owner must validate the original Router/port/peer before every input,
//! publish a whole input group atomically, and close/join physical IO on error.
use super::{native_channel::{self, ChannelPacket, HeaderMode, PacketKind, Records},
    transport_packets::{TransportBytes, TransportError}};
use ring::digest::{digest, SHA256};
use std::{collections::{BTreeMap, BTreeSet, VecDeque}, fmt, time::{Duration, Instant}};

const ACK_BYTES: usize = 64;
const HISTORY: u64 = 4096;
const RX_HISTORY: u64 = 512;
const MAX_PENDING: usize = 256;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_WAIT: Duration = Duration::from_secs(30);
const PROBE_MAGIC: [u8; 4] = [0x89, 0x87, 0x22, 0x83];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChannelError { Configuration, Protocol, Unsupported, Window, Timeout, ClockRollback, PeerReset, Closed }
impl From<TransportError> for ChannelError {
    fn from(error: TransportError) -> Self { match error {
        TransportError::TooLarge => Self::Window, TransportError::UnsupportedKind => Self::Unsupported,
        TransportError::Closed => Self::Closed, _ => Self::Protocol,
    } }
}
struct Queued { body: TransportBytes, deadline: Instant }
struct SentRoot {
    kind: PacketKind, gap: u8, body: Option<TransportBytes>, deadline: Instant,
    sent: Option<Instant>, retries: u16, acknowledged: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint { kind: PacketKind, gap: u8, hash: [u8; 32] }
fn fingerprint(packet: &ChannelPacket<'_>) -> Fingerprint {
    let mut hash = [0; 32]; hash.copy_from_slice(digest(&SHA256, packet.body).as_ref());
    Fingerprint { kind: packet.kind, gap: packet.data_gap, hash }
}
struct ReceivedRoot { previous: u64, fingerprint: Fingerprint, body: TransportBytes, admitted: Instant }

/// Local extended ordinals disambiguate u16 wrap. They are NOT on-wire IDs or
/// authentication. Only a <half-space window of the same incarnation is used.
fn extend(raw: u16, near: u64) -> Result<u64, ChannelError> {
    let delta = raw.wrapping_sub(near as u16) as i16;
    if delta == i16::MIN { return Err(ChannelError::Window); }
    if delta >= 0 { near.checked_add(delta as u64) } else { near.checked_sub(-(delta as i64) as u64) }
        .ok_or(ChannelError::Window)
}

pub(super) struct ChannelCore {
    mtu: usize, body_budget: usize, startup_end: Instant, partial_timeout: Duration, last_now: Instant,
    next_tx: u64, start_tx: u64, last_data_tx: u64, last_ack: u64, start_acked: bool, resets: u8,
    sent: BTreeMap<u64, Option<u64>>, roots: BTreeMap<u64, SentRoot>, queued: VecDeque<Queued>,
    start_rx: Option<u64>, highest_rx: Option<u64>, delivered_rx: Option<u64>,
    seen: BTreeSet<u64>, wire_hashes: BTreeMap<u64, Fingerprint>, root_hashes: BTreeMap<u64, Fingerprint>,
    received: BTreeMap<u64, ReceivedRoot>, records: Records, partial_end: Option<Instant>, gap_end: Option<Instant>,
    ack_pending: bool, reset_reply: Option<u16>, rto: Duration, closed: bool,
}
impl fmt::Debug for ChannelCore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("ChannelCore")
        .field("ready", &self.ready()).field("closed", &self.closed).field("data", &"<redacted>").finish() }
}
impl Drop for ChannelCore { fn drop(&mut self) { self.close(); } }
impl ChannelCore {
    pub fn new(seed: u16, mtu: usize, startup_end: Instant, partial_timeout: Duration, now: Instant) -> Result<Self, ChannelError> {
        if !(128..=1460).contains(&mtu) || partial_timeout.is_zero() || partial_timeout > MAX_WAIT
            || startup_end.checked_duration_since(now).is_none_or(|left| left.is_zero() || left > MAX_WAIT) {
            return Err(ChannelError::Configuration);
        }
        // Native adds0x4000 to a random counter. Restrict this initial draw to
        //14 bits so uninitialised peer ACK0 cannot alias the Start or its short
        //startup retry window. This random value is NOT a cryptographic proof.
        let start = 0x10000 + u64::from((seed & 0x3fff) + 0x4000);
        let mut roots = BTreeMap::new(); roots.insert(start, SentRoot { kind: PacketKind::Start, gap: 0,
            body: Some(TransportBytes::from_bytes(Vec::new())), deadline: startup_end, sent: None, retries: 0, acknowledged: false });
        Ok(Self { mtu, body_budget: mtu - 8 - ACK_BYTES - 2 - 2, startup_end, partial_timeout, last_now: now,
            next_tx: start, start_tx: start, last_data_tx: start, last_ack: start - 1, start_acked: false, resets: 0,
            sent: BTreeMap::new(), roots, queued: VecDeque::new(), start_rx: None, highest_rx: None, delivered_rx: None,
            seen: BTreeSet::new(), wire_hashes: BTreeMap::new(), root_hashes: BTreeMap::new(), received: BTreeMap::new(),
            records: Records::default(), partial_end: None, gap_end: None, ack_pending: false, reset_reply: None,
            rto: Duration::from_millis(200), closed: false })
    }
    pub fn ready(&self) -> bool { !self.closed && self.start_acked && self.start_rx.is_some() }
    pub fn close(&mut self) {
        self.closed = true; self.sent.clear(); self.roots.clear(); self.queued.clear(); self.received.clear();
        self.seen.clear(); self.wire_hashes.clear(); self.root_hashes.clear(); self.records.close();
        self.partial_end = None; self.gap_end = None; self.ack_pending = false; self.reset_reply = None;
    }
    pub fn has_partial(&self) -> bool { self.records.has_partial() || !self.received.is_empty() }
    pub fn set_echoed_delay(&mut self, milliseconds: Option<u16>) {
        // Conservative local retry policy. Timing changes NEVER renew a root,
        //startup, partial-record or caller deadline.
        self.rto = Duration::from_millis(milliseconds.map_or(200, |ms| (u64::from(ms) * 2 + 100).clamp(200, 4000)));
    }
    fn check(&mut self, now: Instant) -> Result<(), ChannelError> {
        if self.closed { return Err(ChannelError::Closed); }
        if now < self.last_now { return Err(ChannelError::ClockRollback); }
        self.last_now = now;
        if !self.ready() && now >= self.startup_end || self.partial_end.is_some_and(|end| now >= end)
            || self.gap_end.is_some_and(|end| now >= end)
            || self.queued.iter().any(|item| now >= item.deadline)
            || self.roots.values().any(|item| now >= item.deadline && item.kind != PacketKind::Start)
            || !self.start_acked && self.roots.get(&self.start_tx).is_some_and(|item| now >= item.deadline) {
            return Err(ChannelError::Timeout);
        }
        Ok(())
    }
    pub fn tick(&mut self, now: Instant) -> Result<(), ChannelError> {
        let result = self.check(now); if result.is_err() { self.close(); } result
    }
    pub fn next_wake(&self) -> Option<Instant> {
        if self.closed { return None; }
        let mut end = (!self.ready()).then_some(self.startup_end);
        for value in [self.partial_end, self.gap_end].into_iter().flatten() { end = Some(end.map_or(value, |old| old.min(value))); }
        if self.ack_pending || self.reset_reply.is_some() || self.ready() && !self.queued.is_empty() && self.roots.len() < MAX_PENDING {
            return Some(self.last_now);
        }
        for item in self.roots.values() {
            if item.kind != PacketKind::Start || !self.start_acked { end = Some(end.map_or(item.deadline, |old| old.min(item.deadline))); }
            if !item.acknowledged {
                let retry = item.sent.and_then(|time| time.checked_add(self.rto)).unwrap_or(self.last_now);
                end = Some(end.map_or(retry, |old| old.min(retry)));
            }
        }
        for item in &self.queued { end = Some(end.map_or(item.deadline, |old| old.min(item.deadline))); }
        end
    }
    fn retained_send_bytes(&self) -> usize {
        self.queued.iter().map(|x| x.body.expose_to_transport().len() + 2).sum::<usize>()
            + self.roots.values().filter_map(|x| x.body.as_ref()).map(|x| x.expose_to_transport().len()).sum::<usize>()
    }
    /// Reserve all encrypted fragments before committing a directional record
    ///encoder. A capacity error here has consumed no plaintext/cipher state.
    pub fn queue_record(&mut self, record: &[u8], deadline: Instant, now: Instant) -> Result<(), ChannelError> {
        self.tick(now)?;
        if record.is_empty() || deadline.checked_duration_since(now).is_none_or(|left| left.is_zero() || left > MAX_WAIT) {
            return Err(ChannelError::Configuration);
        }
        let count = record.len().div_ceil(self.body_budget - 1);
        let bytes = record.len().checked_add(count.checked_mul(3).ok_or(ChannelError::Window)?).ok_or(ChannelError::Window)?;
        if count > MAX_PENDING.saturating_sub(self.queued.len() + self.roots.len())
            || bytes > MAX_BYTES.saturating_sub(self.retained_send_bytes()) { return Err(ChannelError::Window); }
        let pieces = native_channel::encode_record(record, self.body_budget)?;
        self.queued.extend(pieces.into_iter().map(|body| Queued { body, deadline })); Ok(())
    }
    fn ack_fields(&self) -> (u16, u16, Vec<u8>) {
        let Some(high) = self.highest_rx else { return (0, 0, Vec::new()); };
        // Always a full selective window, no guessed cumulative prefix. This
        //also re-ACKs a late hole when ack_sequence stays unchanged (F562E0).
        let start = ((high as u16).wrapping_sub(1) & !7).wrapping_sub((8 * (ACK_BYTES - 1)) as u16);
        let count = (high as u16).wrapping_sub(start) as usize; let mut mask = vec![0; ACK_BYTES];
        for bit in 0..count {
            if extend(start.wrapping_add(bit as u16), high).ok().is_some_and(|seq| self.seen.contains(&seq)) {
                mask[bit / 8] |= 1 << (bit & 7);
            }
        }
        (high as u16, self.delivered_rx.unwrap_or_else(|| self.start_rx.unwrap_or(high) - 1) as u16, mask)
    }
    fn frame(&mut self, kind: PacketKind, gap: u8, body: &[u8], root: Option<u64>) -> Result<TransportBytes, ChannelError> {
        let (ack, delivered, mask) = self.ack_fields(); let sequence = self.next_tx;
        // Even ACK-only traffic must not age an original DATA reference out of
        //the native bounded history or alias it after a whole u16 revolution.
        if sequence.checked_sub(self.last_data_tx).is_none_or(|age| age >= HISTORY)
            || self.roots.keys().next().is_some_and(|old| sequence.saturating_sub(*old) >= HISTORY) {
            return Err(ChannelError::Window);
        }
        let bytes = native_channel::encode_packet(HeaderMode::Basic, &ChannelPacket { timestamp: None,
            sequence: sequence as u16, kind, data_gap: gap, ack_sequence: ack, delivered_sequence: delivered,
            inclusive_ack: false, ack_mask: &mask, body })?;
        if bytes.expose_to_transport().len() > self.mtu { return Err(ChannelError::Window); }
        self.next_tx = self.next_tx.checked_add(1).ok_or(ChannelError::Window)?;
        self.sent.insert(sequence, root);
        let floor = self.next_tx.saturating_sub(HISTORY); self.sent.retain(|key, _| *key >= floor);
        self.ack_pending = false; Ok(bytes)
    }
    pub fn next_packet(&mut self, now: Instant) -> Result<Option<TransportBytes>, ChannelError> {
        let result = self.transmit(now); if result.is_err() { self.close(); } result
    }
    fn transmit(&mut self, now: Instant) -> Result<Option<TransportBytes>, ChannelError> {
        self.check(now)?;
        if let Some(sequence) = self.reset_reply.take() {
            // F55D44: a missing-incarnation request is deliberately not a new
            //send-history entry and not a normal empty-body ACK/Probe.
            return Ok(Some(native_channel::encode_packet(HeaderMode::Basic, &ChannelPacket { timestamp: None,
                sequence, kind: PacketKind::Probe, data_gap: 0, ack_sequence: sequence, delivered_sequence: 0,
                inclusive_ack: false, ack_mask: &[0; 4], body: &[] })?));
        }
        let retry = self.roots.iter().find(|(_, item)| !item.acknowledged
            && item.sent.is_none_or(|sent| now.duration_since(sent) >= self.rto)).map(|(&seq, _)| seq);
        if let Some(root) = retry {
            let item = self.roots.get(&root).ok_or(ChannelError::Protocol)?;
            if item.retries >= 128 { return Err(ChannelError::Timeout); }
            let original = ChannelPacket { timestamp: None, sequence: root as u16, kind: item.kind, data_gap: item.gap,
                ack_sequence: 0, delivered_sequence: 0, inclusive_ack: false, ack_mask: &[],
                body: item.body.as_ref().ok_or(ChannelError::Protocol)?.expose_to_transport() };
            let sent = item.sent.is_some(); let gap = item.gap;
            if sent && self.next_tx.checked_sub(root).is_none_or(|age| age == 0 || age >= 0x4000)
                || !sent && self.next_tx != root { return Err(ChannelError::Window); }
            let body = if sent { native_channel::retransmission_body(&original, self.next_tx as u16)? }
                else { TransportBytes::from_bytes(original.body.to_vec()) };
            let kind = if sent { PacketKind::Skip } else { item.kind };
            let packet = self.frame(kind, gap, body.expose_to_transport(), Some(root))?;
            let item = self.roots.get_mut(&root).ok_or(ChannelError::Protocol)?; item.sent = Some(now); item.retries += 1;
            return Ok(Some(packet));
        }
        if self.ready() && self.roots.len() < MAX_PENDING {
            if let Some(queued) = self.queued.pop_front() {
                let sequence = self.next_tx;
                let distance = sequence.checked_sub(self.last_data_tx.checked_add(1).ok_or(ChannelError::Window)?).ok_or(ChannelError::Protocol)?;
                if distance >= HISTORY { return Err(ChannelError::Window); }
                let gap = distance.min(31) as u8; let prefix = if gap == 31 {2} else {0};
                let mut body = Vec::with_capacity(queued.body.expose_to_transport().len() + prefix);
                if gap == 31 { body.extend_from_slice(&(self.last_data_tx as u16).to_be_bytes()); }
                body.extend_from_slice(queued.body.expose_to_transport());
                let body = TransportBytes::from_bytes(body);
                let packet = self.frame(PacketKind::Data, gap, body.expose_to_transport(), Some(sequence))?;
                self.roots.insert(sequence, SentRoot { kind: PacketKind::Data, gap, body: Some(body), deadline: queued.deadline,
                    sent: Some(now), retries: 1, acknowledged: false }); self.last_data_tx = sequence;
                return Ok(Some(packet));
            }
        }
        if self.ack_pending && self.start_rx.is_some() { return Ok(Some(self.frame(PacketKind::Probe, 0, &PROBE_MAGIC, None)?)); }
        Ok(None)
    }
    fn acknowledge(&mut self, packet: &ChannelPacket<'_>) -> Result<(), ChannelError> {
        let last = self.next_tx.checked_sub(1).ok_or(ChannelError::Protocol)?;
        // A half-space/overflow ACK cannot name our bounded captured history.
        //Native F562E0 ignores unknown anchors rather than treating ACK0 from
        //an uninitialised peer as a new receive incarnation or a fatal reset.
        let Ok(high) = extend(packet.ack_sequence, last) else { return Ok(()); };
        // Unknown, future and pre-incarnation ACKs cannot touch any original
        //pending DATA even when their bitmap happens to name a live sequence.
        if !self.sent.contains_key(&high) || high < self.last_ack { return Ok(()); }
        let delivered = extend(packet.delivered_sequence, last)?;
        // F52D24 copies highest-received2196 and DATA-delivered2192 together;
        //an original DATA delivery cursor can never exceed that matched high.
        if delivered > high { return Err(ChannelError::Protocol); }
        // Native0x80 compacts the NEW prefix after the previous highest ACK;
        //it is not TCP's acknowledgement of every older hole. A late hole
        //needs a selective bit, its real retransmission alias, or DATA delivery.
        let acked = native_channel::acknowledged_sequences(packet, self.last_ack as u16)?;
        for sequence in acked {
            let index = extend(sequence, high)?;
            if let Some(Some(root)) = self.sent.get(&index) {
                if let Some(item) = self.roots.get_mut(root) {
                    item.acknowledged = true; item.body = None;
                    if *root == self.start_tx { self.start_acked = true; }
                }
            }
        }
        self.last_ack = high;
        // Delivery ACK is an original DATA/Start cursor, not arbitrary packet
        //numbers, RPC completion, or provider acceptance. Bound by real roots.
        if self.roots.contains_key(&delivered) {
            let keys: Vec<_> = self.roots.range(..=delivered).map(|(&key, _)| key).collect();
            for key in keys { self.roots.remove(&key); if key == self.start_tx { self.start_acked = true; } }
        }
        Ok(())
    }
    fn restart_before_ready(&mut self) -> Result<(), ChannelError> {
        if self.ready() || self.resets != 0 || self.roots.values().any(|root| root.kind == PacketKind::Data) {
            return Err(ChannelError::PeerReset);
        }
        // F560B8 resets sender and adds0x4000. Only a single pre-DATA reset is
        //allowed here; no old business record or resumed cipher is replayed.
        self.resets += 1; let start = self.next_tx.checked_add(0x4000).ok_or(ChannelError::Window)?;
        self.next_tx = start; self.start_tx = start; self.last_data_tx = start; self.last_ack = start - 1;
        self.start_acked = false; self.sent.clear(); self.roots.clear();
        self.roots.insert(start, SentRoot { kind: PacketKind::Start, gap: 0, body: Some(TransportBytes::from_bytes(Vec::new())),
            deadline: self.startup_end, sent: None, retries: 0, acknowledged: false }); Ok(())
    }
    fn validate_probe(packet: &ChannelPacket<'_>) -> Result<(), ChannelError> {
        if packet.data_gap != 0 || packet.body.get(..4) != Some(PROBE_MAGIC.as_slice()) { return Err(ChannelError::Protocol); }
        let mut rest = &packet.body[4..];
        while !rest.is_empty() {
            if rest == [0xff] { break; }
            if rest[0] != 0 || rest.len() < 5 { return Err(ChannelError::Unsupported); }
            // Consume the native hint even when zero/negative (native ignores
            //those). No peer value can enlarge our local queue or deadline.
            let _hint = u32::from_be_bytes(rest[1..5].try_into().map_err(|_| ChannelError::Protocol)?);
            rest = &rest[5..];
        }
        Ok(())
    }
    pub fn on_packet(&mut self, bytes: &[u8], now: Instant) -> Result<Vec<TransportBytes>, ChannelError> {
        self.on_packet_admitted(bytes, now, now)
    }
    pub fn on_packet_admitted(&mut self, bytes: &[u8], admitted: Instant, now: Instant) -> Result<Vec<TransportBytes>, ChannelError> {
        let result = self.receive(bytes, admitted, now); if result.is_err() { self.close(); } result
    }
    fn receive(&mut self, bytes: &[u8], admitted: Instant, now: Instant) -> Result<Vec<TransportBytes>, ChannelError> {
        self.check(now)?;
        if admitted > now { return Err(ChannelError::ClockRollback); }
        // Peer segmentation has its own MTU. The local transmit budget is not
        //a guessed inbound MTU negotiation; receive allocation stays bounded.
        if bytes.len() > MAX_BYTES { return Err(ChannelError::Window); }
        let packet = native_channel::decode_packet(HeaderMode::Basic, bytes)?;
        if packet.kind == PacketKind::Probe && packet.data_gap == 0 && !packet.inclusive_ack
            && packet.ack_mask == [0; 4] {
            // F53EE8 calls F562E0 BEFORE F53F40 removes the ACK mask. The
            //native marker sees these four zeros irrespective of trailing
            //body; it is NOT a valid ordinary Probe with a zero bitmap.
            if !packet.body.is_empty() || packet.delivered_sequence != 0 || packet.ack_sequence != packet.sequence { return Err(ChannelError::Protocol); }
            self.restart_before_ready()?; return Ok(Vec::new());
        }
        if packet.kind == PacketKind::Fec { return Err(ChannelError::Unsupported); }
        let (original, back) = native_channel::original_packet_distance(&packet)?;
        if original.kind == PacketKind::Probe { Self::validate_probe(&original)?; }
        if original.kind == PacketKind::Start && original.data_gap != 0 { return Err(ChannelError::Protocol); }
        if back >= 0x4000 { return Err(ChannelError::Window); }
        let back = back as u64;
        // Fully validate structural DATA fields BEFORE processing any ACK.
        let data = if original.kind == PacketKind::Data { Some(native_channel::previous_data(&original)?) } else { None };
        self.acknowledge(&packet)?;
        let (wire, root) = if let Some(high) = self.highest_rx {
            let wire = extend(packet.sequence, high)?;
            if wire > high.saturating_add(HISTORY) || wire.saturating_add(768) < high { return Err(ChannelError::Window); }
            (wire, wire.checked_sub(back).ok_or(ChannelError::Window)?)
        } else if original.kind == PacketKind::Start {
            let root = 0x10000 + u64::from(original.sequence); (root + back, root)
        } else {
            self.reset_reply = Some(packet.sequence); return Ok(Vec::new());
        };
        if original.kind == PacketKind::Start {
            if self.start_rx.is_some_and(|old| old != root) { return Err(ChannelError::PeerReset); }
            if self.start_rx.is_none() { self.start_rx = Some(root); self.delivered_rx = Some(root - 1); }
        }
        let start = self.start_rx.ok_or(ChannelError::Protocol)?;
        if root < start { return Err(ChannelError::Window); }
        let wire_fingerprint = fingerprint(&packet);
        if self.wire_hashes.get(&wire).is_some_and(|old| old != &wire_fingerprint) { return Err(ChannelError::Protocol); }
        self.wire_hashes.insert(wire, wire_fingerprint);
        let high = self.highest_rx.map_or(wire, |old| old.max(wire)); self.highest_rx = Some(high);
        self.seen.insert(wire); self.seen.insert(root);
        let floor = high.saturating_sub(RX_HISTORY - 1);
        self.seen.retain(|seq| *seq >= floor); self.wire_hashes.retain(|seq, _| *seq >= floor); self.root_hashes.retain(|seq, _| *seq >= floor);
        if original.kind != PacketKind::Probe {
            self.ack_pending = true; let fingerprint = fingerprint(&original);
            if self.root_hashes.get(&root).is_some_and(|old| old != &fingerprint)
                || self.received.get(&root).is_some_and(|old| old.fingerprint != fingerprint) { return Err(ChannelError::Protocol); }
            let delivered = self.delivered_rx.ok_or(ChannelError::Protocol)?;
            if root <= delivered && root >= floor && !self.root_hashes.contains_key(&root) {
                // A recent sequence skipped as NON-data cannot later become a
                //new DATA root and silently alter the ordered record stream.
                return Err(ChannelError::Protocol);
            }
            if root > delivered && !self.received.contains_key(&root) {
                let (previous, body) = if let Some((previous, body)) = data { (extend(previous, root)?, body) } else { (root - 1, original.body) };
                if previous >= root || original.kind == PacketKind::Data && (previous < start
                    || previous < self.delivered_rx.ok_or(ChannelError::Protocol)?) { return Err(ChannelError::Protocol); }
                if self.received.values().any(|old| old.previous == previous) { return Err(ChannelError::Protocol); }
                use std::ops::Bound::Excluded;
                // DATA edges declare every interior sequence NON-data. Check
                //both crossing directions before any buffered record drains.
                if self.received.range((Excluded(previous), Excluded(root))).next().is_some()
                    || self.root_hashes.range((Excluded(previous), Excluded(root))).next().is_some()
                    || self.received.iter().any(|(&end, old)| old.previous < root && root < end) {
                    return Err(ChannelError::Protocol);
                }
                let retained = self.records.retained_bytes() + self.received.values().map(|old| old.body.expose_to_transport().len()).sum::<usize>();
                if self.received.len() >= MAX_PENDING || body.len() > MAX_BYTES.saturating_sub(retained) { return Err(ChannelError::Window); }
                self.received.insert(root, ReceivedRoot { previous, fingerprint, body: TransportBytes::from_bytes(body.to_vec()), admitted });
            }
        }
        let carried_gap = self.gap_end;
        let mut output = Vec::new(); let mut output_bytes = 0usize;
        if self.ready() {
            loop {
                let delivered = self.delivered_rx.ok_or(ChannelError::Protocol)?;
                let next = self.received.iter().find(|(_, value)| value.previous == delivered).map(|(&key, _)| key);
                let Some(next) = next else { break; };
                let item = self.received.remove(&next).ok_or(ChannelError::Protocol)?;
                let records = self.records.on_payload(item.body.expose_to_transport())?;
                if !records.is_empty() || !self.records.has_partial() { self.partial_end = None; }
                if self.records.has_partial() && self.partial_end.is_none() {
                    let end = item.admitted.checked_add(self.partial_timeout).ok_or(ChannelError::Window)?;
                    // Solving a DATA gap cannot buy a fresh partial budget for
                    //the old buffered message. Retain its earliest admission.
                    self.partial_end = Some(carried_gap.map_or(end, |old| old.min(end)));
                }
                for record in records {
                    output_bytes = output_bytes.checked_add(record.expose_to_transport().len()).ok_or(ChannelError::Window)?;
                    if output.len() >= MAX_RECORDS || output_bytes > MAX_BYTES { return Err(ChannelError::Window); }
                    output.push(record);
                }
                self.root_hashes.insert(next, item.fingerprint); self.delivered_rx = Some(next); self.ack_pending = true;
            }
        }
        let first_gap = self.received.values().filter(|item| !(item.fingerprint.kind == PacketKind::Start
            && item.body.expose_to_transport().is_empty() && Some(item.previous) == self.delivered_rx))
            .map(|item| item.admitted).min();
        if let Some(first) = first_gap {
            if self.gap_end.is_none() { self.gap_end = first.checked_add(self.partial_timeout); }
        } else { self.gap_end = None; }
        if self.records.has_partial() && self.partial_end.is_none() || first_gap.is_some() && self.gap_end.is_none() { return Err(ChannelError::Window); }
        if output_bytes + self.records.retained_bytes() + self.received.values().map(|item| item.body.expose_to_transport().len()).sum::<usize>() > MAX_BYTES {
            return Err(ChannelError::Window);
        }
        // A delayed, already-expired first partial/gap cannot be published or
        //ACKed just because its protocol parsing happened later than receipt.
        self.check(now)?;
        Ok(output)
    }
}

#[cfg(test)]
#[path = "reliable_channel_tests.rs"]
mod tests;
