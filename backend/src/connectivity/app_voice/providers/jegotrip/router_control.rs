//! Native Router port-zero controls, distinct from kind2/3 path timing and
//! from kind1 Common RPC. D47FF8/D47F08 use packet kind0, packet type0, and one
//! byte 0/1. D4C1AC decodes opcode4 followed by a type-0 stream (no RPC footer).
//! Pure parsing only: no port grant, SDK authentication or endpoint selection.
use super::rpc_stream::{CodecError, Reader};
use std::{collections::BTreeMap, fmt};

const MAX_CONTROL_BYTES: usize = 8192;
const MAX_REPORT_ENTRIES: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PeerWords(pub [u32; 4]);
impl fmt::Debug for PeerWords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PeerWords(<redacted>)") }
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct RouterReport {
    pub levels: [u8; 4],
    pub networks: BTreeMap<u32, [u16; 4]>,
    pub peers: BTreeMap<PeerWords, [u16; 4]>,
}
impl RouterReport {
    /// Native opcode4 updates existing original-path records; absent keys are
    /// not deletions. Merge atomically under a bounded per-path table, not an
    /// unbounded global cache or a mutable current-account endpoint lookup.
    pub fn merge_from(&mut self, delta: &Self) -> Result<(), CodecError> {
        let mut next = self.clone(); next.levels = delta.levels;
        for (&key, &words) in &delta.networks {
            if !next.networks.contains_key(&key) && next.networks.len() == MAX_REPORT_ENTRIES { return Err(CodecError::TooLarge); }
            next.networks.insert(key, words);
        }
        for (&key, &words) in &delta.peers {
            if !next.peers.contains_key(&key) && next.peers.len() == MAX_REPORT_ENTRIES { return Err(CodecError::TooLarge); }
            next.peers.insert(key, words);
        }
        *self = next; Ok(())
    }
}
impl fmt::Debug for RouterReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterReport(<redacted>)") }
}
#[derive(Clone, PartialEq, Eq)]
pub(super) enum NodeControl { Probe, Alive, Report(RouterReport) }
impl fmt::Debug for NodeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NodeControl(<redacted>)") }
}

fn four_shorts(reader: &mut Reader<'_>) -> Result<[u16; 4], CodecError> {
    Ok([reader.u16()?, reader.u16()?, reader.u16()?, reader.u16()?])
}
fn count(reader: &mut Reader<'_>, minimum_bytes: usize) -> Result<usize, CodecError> {
    let count = reader.count(minimum_bytes)?;
    if count > MAX_REPORT_ENTRIES { return Err(CodecError::TooLarge); }
    Ok(count)
}

pub(super) fn decode_node_control(payload: &[u8]) -> Result<NodeControl, CodecError> {
    if payload.len() > MAX_CONTROL_BYTES { return Err(CodecError::TooLarge); }
    let (&opcode, body) = payload.split_first().ok_or(CodecError::Truncated)?;
    match opcode {
        0 | 1 => {
            if !body.is_empty() { return Err(CodecError::TrailingData); }
            Ok(if opcode == 0 { NodeControl::Probe } else { NodeControl::Alive })
        }
        4 => {
            // E86DA0 selects StreamData0; stream2 remains explicitly unsupported.
            let mut reader = Reader::new(body)?;
            // E4BAC0 reads FOUR BYTES, not four shorts. Read them together only
            // to preserve their exact byte order; this is not an integer ID.
            let levels = reader.u32()?.to_be_bytes();
            let mut networks = BTreeMap::new();
            for _ in 0..count(&mut reader, 12)? {
                let key = reader.u32()?;
                let words = four_shorts(&mut reader)?; // E524AC / E4BEDC
                if networks.insert(key, words).is_some() { return Err(CodecError::DuplicateKey); }
            }
            let mut peers = BTreeMap::new();
            for _ in 0..count(&mut reader, 24)? {
                // E5437C / E4C904: four full-width words followed by four u16s.
                // They remain private report data until original-path route
                // matching validates the requested destination and next hop.
                let key = PeerWords([reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?]);
                let words = four_shorts(&mut reader)?;
                if peers.insert(key, words).is_some() { return Err(CodecError::DuplicateKey); }
            }
            reader.finish()?;
            Ok(NodeControl::Report(RouterReport { levels, networks, peers }))
        }
        _ => Err(CodecError::InvalidField),
    }
}

#[cfg(test)]
#[path = "router_control_tests.rs"]
mod tests;
