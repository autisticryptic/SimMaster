//! Bounded original-path clock echo, delay and loss observations.
//! Recovered D66270 receive/D63960 transmit semantics, not SDK/Channel ACKs.
//! Duplicate Router packets remain deliverable elsewhere; only statistics skip
//! duplicates. Peer clock resets never renew an RPC or owner deadline.
use super::{native_router::{ReceiveObservation, SequenceBookkeeping}, packet_transport::RouterTiming, router_packets::RouterHeader};
use std::{fmt, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TimingError { Closed, ClockRollback, Overflow }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TimingObservation { pub reset_receiver: bool, pub feedback_due: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PathMetrics { pub echoed_delay_ms: Option<u16>, pub receive_loss_percent: u8, pub peer_loss: Option<u8> }

#[derive(Clone, Copy, Default)]
struct Sample { count: u64, sum: u64, deviation: u64 }
struct Samples { slots: [Sample; 4], head: usize, since: Instant }
impl Samples {
    fn new(now: Instant, value: u64) -> Self {
        let mut slots = [Sample::default(); 4]; slots[0] = Sample { count: 1, sum: value, deviation: 0 };
        Self { slots, head: 0, since: now }
    }
    fn push(&mut self, value: u64, deviation: u64, now: Instant) -> Result<(), TimingError> {
        // Native advances ONE sample bucket after >=1000ms, not one bucket for
        // every elapsed idle second. It does not manufacture empty samples.
        if now.duration_since(self.since) >= Duration::from_millis(1000) {
            self.head = (self.head + 1) & 3; self.slots[self.head] = Sample::default(); self.since = now;
        }
        let slot = &mut self.slots[self.head];
        slot.count = slot.count.checked_add(1).ok_or(TimingError::Overflow)?;
        slot.sum = slot.sum.checked_add(value).ok_or(TimingError::Overflow)?;
        slot.deviation = slot.deviation.checked_add(deviation).ok_or(TimingError::Overflow)?;
        Ok(())
    }
    fn means(&self) -> Result<(u64, u64), TimingError> {
        let mut total = Sample::default();
        for slot in self.slots {
            total.count = total.count.checked_add(slot.count).ok_or(TimingError::Overflow)?;
            total.sum = total.sum.checked_add(slot.sum).ok_or(TimingError::Overflow)?;
            total.deviation = total.deviation.checked_add(slot.deviation).ok_or(TimingError::Overflow)?;
        }
        if total.count == 0 { return Err(TimingError::Overflow); }
        Ok((total.sum / total.count, total.deviation / total.count))
    }
}
struct PeerClock { at: Instant, word: u16, lag: Samples }
impl PeerClock {
    fn advance(&mut self, now: Instant) -> Result<u16, TimingError> {
        let elapsed = now.checked_duration_since(self.at).ok_or(TimingError::ClockRollback)?;
        let seconds = elapsed.as_secs();
        if seconds != 0 {
            self.at = self.at.checked_add(Duration::from_secs(seconds)).ok_or(TimingError::Overflow)?;
            self.word = self.word.wrapping_add(((seconds % 65536) * 999) as u16);
        }
        Ok(self.word.wrapping_add(now.duration_since(self.at).as_millis() as u16))
    }
}

#[derive(Clone, Copy, Default)]
struct LossBucket { expected: u64, received: u64 }
struct LossWindow { slots: [LossBucket; 64], head: usize, since: Instant, highest: u16, last_peer_send: u16 }
impl LossWindow {
    fn new(sequence: u16, peer_send: u16, now: Instant) -> Self {
        let mut slots = [LossBucket::default(); 64]; slots[0] = LossBucket { expected: 1, received: 1 };
        Self { slots, head: 0, since: now, highest: sequence, last_peer_send: peer_send }
    }
    fn observe(&mut self, sequence: u16, peer_send: u16, now: Instant) -> Result<(), TimingError> {
        let distance = sequence.wrapping_sub(self.highest) as i16 as i32;
        if distance > 0 {
            let distance = distance as u64;
            if now.duration_since(self.since) >= Duration::from_millis(1000) {
                self.slots[self.head].expected = self.slots[self.head].expected.checked_add(distance - 1).ok_or(TimingError::Overflow)?;
                self.head = (self.head + 1) & 63; self.since = now;
                self.slots[self.head] = LossBucket { expected: 1, received: 1 };
            } else {
                let slot = &mut self.slots[self.head];
                slot.expected = slot.expected.checked_add(distance).ok_or(TimingError::Overflow)?;
                slot.received = slot.received.checked_add(1).ok_or(TimingError::Overflow)?;
            }
            self.highest = sequence; self.last_peer_send = peer_send;
        } else {
            let mut remaining = i128::from(distance);
            for age in 0..64 {
                let index = (self.head + 64 - age) & 63;
                let expected = self.slots[index].expected;
                if expected == 0 { break; }
                remaining += i128::from(expected);
                if remaining >= 0 {
                    self.slots[index].received = self.slots[index].received.checked_add(1).ok_or(TimingError::Overflow)?;
                    break;
                }
            }
        }
        Ok(())
    }
    fn percent(&self) -> Result<u8, TimingError> {
        let mut maximum = 0u64;
        for length in [8, 16, 32, 64] {
            let (mut expected, mut received) = (0u64, 0u64);
            for age in 0..length {
                let slot = self.slots[(self.head + 64 - age) & 63];
                expected = expected.checked_add(slot.expected).ok_or(TimingError::Overflow)?;
                received = received.checked_add(slot.received).ok_or(TimingError::Overflow)?;
            }
            if expected != 0 {
                // Conservative local clamp: late packets preceding the initial
                // observation must not turn negative native loss into byte255.
                let lost = expected.saturating_sub(received);
                let percent = (u128::from(lost) * 100 / u128::from(expected)) as u64;
                maximum = maximum.max(percent.min(100));
            }
        }
        Ok(maximum as u8)
    }
}

pub(super) struct PathTiming {
    start: Instant, last_now: Instant, seen: SequenceBookkeeping, clock: Option<PeerClock>,
    loss: Option<LossWindow>, echoed: Option<Samples>, peer_loss: Option<u8>, unsignalled: u16, closed: bool,
}
impl fmt::Debug for PathTiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PathTiming(<redacted>)") }
}
impl PathTiming {
    pub fn new(start: Instant) -> Self {
        Self { start, last_now: start, seen: SequenceBookkeeping::default(), clock: None, loss: None,
            echoed: None, peer_loss: None, unsignalled: 0, closed: false }
    }
    pub fn close(&mut self) {
        self.clock = None; self.loss = None; self.echoed = None; self.peer_loss = None;
        self.unsignalled = 0; self.seen = SequenceBookkeeping::default(); self.closed = true;
    }
    fn time(&mut self, now: Instant) -> Result<(), TimingError> {
        if self.closed { return Err(TimingError::Closed); }
        if now < self.last_now { self.close(); return Err(TimingError::ClockRollback); }
        self.last_now = now; Ok(())
    }
    fn reset_receive(&mut self, header: RouterHeader, now: Instant) {
        self.seen = SequenceBookkeeping::default(); self.seen.observe(header.sequence);
        self.clock = Some(PeerClock { at: now, word: header.word1, lag: Samples::new(now, 0) });
        self.loss = Some(LossWindow::new(header.sequence, header.word1, now)); self.unsignalled = 0;
        // Native receive-window reset does not itself discard existing echoed
        // delay; the current packet's word2 below determines its availability.
    }
    pub fn observe(&mut self, header: RouterHeader, now: Instant) -> Result<TimingObservation, TimingError> {
        self.time(now)?;
        let result = self.observe_inner(header, now);
        if result.is_err() { self.close(); } result
    }
    fn observe_inner(&mut self, header: RouterHeader, now: Instant) -> Result<TimingObservation, TimingError> {
        let observation = self.seen.observe(header.sequence);
        if observation == ReceiveObservation::Duplicate { return Ok(TimingObservation { reset_receiver: false, feedback_due: false }); }
        let mut reset = observation == ReceiveObservation::FirstOrReset;
        if !reset {
            let clock = self.clock.as_mut().ok_or(TimingError::Overflow)?;
            let predicted = clock.advance(now)?;
            let mut lag = predicted.wrapping_sub(header.word1) as i16 as i32;
            if !(-6000..=6000).contains(&lag) { reset = true; }
            else {
                if lag < 0 { clock.at = now; clock.word = header.word1; lag = 0; }
                let old_mean = clock.lag.means()?.0;
                let deviation = u64::from(lag as u32).abs_diff(old_mean);
                clock.lag.push(lag as u64, deviation, now)?;
                self.loss.as_mut().ok_or(TimingError::Overflow)?.observe(header.sequence, header.word1, now)?;
            }
        }
        if reset { self.reset_receive(header, now); }
        if header.word2 == 0 { self.echoed = None; }
        else {
            let local = now.duration_since(self.start).as_millis() as u16;
            let delay = (local.wrapping_sub(header.word2) as i16 as i32).clamp(0, 16000) as u64;
            if let Some(samples) = &mut self.echoed { samples.push(delay, 0, now)?; }
            else { self.echoed = Some(Samples::new(now, delay)); }
            self.peer_loss = Some(header.byte6);
        }
        let mut feedback_due = false;
        if header.packet_kind <= 2 {
            self.unsignalled += 1;
            if self.unsignalled >= 100 { self.unsignalled = 0; feedback_due = true; }
        }
        Ok(TimingObservation { reset_receiver: reset, feedback_due })
    }
    /// Called when an original path packet is prepared for transmission. No
    /// packets are sent by this module; it neither owns IO nor extends budgets.
    pub fn note_transmission(&mut self) { self.unsignalled = 0; }
    pub fn timing(&mut self, now: Instant) -> Result<RouterTiming, TimingError> {
        self.time(now)?;
        let result = self.timing_inner(now); if result.is_err() { self.close(); } result
    }
    fn timing_inner(&mut self, now: Instant) -> Result<RouterTiming, TimingError> {
        let word1 = now.duration_since(self.start).as_millis() as u16;
        let Some(clock) = &mut self.clock else { return Ok(RouterTiming { word1, word2: 0, byte6: 0 }); };
        let predicted = clock.advance(now)?; let (lag, deviation) = clock.lag.means()?;
        let word2 = predicted.wrapping_sub(lag as u16).wrapping_sub(deviation as u16).max(1);
        let loss = self.loss.as_ref().ok_or(TimingError::Overflow)?.percent()?;
        Ok(RouterTiming { word1, word2, byte6: loss })
    }
    pub fn metrics(&self) -> Result<PathMetrics, TimingError> {
        if self.closed { return Err(TimingError::Closed); }
        Ok(PathMetrics { echoed_delay_ms: self.echoed.as_ref().map(|samples| samples.means().map(|v| v.0.min(16000) as u16)).transpose()?,
            receive_loss_percent: self.loss.as_ref().map(LossWindow::percent).transpose()?.unwrap_or(0), peer_loss: self.peer_loss })
    }
    pub fn probe_echo(&self) -> Option<(u16, u16)> {
        if self.closed { return None; }
        self.loss.as_ref().map(|loss| (loss.last_peer_send, loss.highest))
    }
}

#[cfg(test)]
#[path = "router_timing_tests.rs"]
mod tests;
