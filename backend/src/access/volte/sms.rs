//! VoLTE SMS flow: MT (receive) reassembly + dedup + RP-ACK, and MO (send)
//! submission building.
//!
//! Clean-room from 3GPP TS 24.011 (RP), TS 23.040 (TPDU), TS 24.341 (SMS over
//! IP). The 3GPP codec itself is reused from `vowifi::sms` (transport-agnostic):
//!   - MT: `parse_mt_rp_data` -> `MtSmsDeliver`
//!   - MO: `build_single_part_mo_submission` -> `MoSmsSubmission`
//!   - RP-ACK: `build_network_rp_ack`, `classify_rp_ack`
//!
//! This module adds the VoLTE-specific orchestration: multipart segment
//! reassembly, cross-frame dedup keys, and the persistence marker/transport tag
//! (`volte_ims` / `volte-mt:<key>`) so stored MT SMS dedup deterministically.

use std::collections::HashMap;

use crate::access::vowifi::sms::{parse_mt_rp_data, MtSmsDeliver, SmsEncodingError};

/// Transport tag stored on the shared `sms_messages` row (db `transport`
/// column), distinguishing VoLTE-delivered SMS from modem/vowifi ones.
pub const TRANSPORT_TAG: &str = "volte_ims";
/// Prefix for the synthetic dedup marker stored in the `pdu` column.
pub const MT_MARKER_PREFIX: &str = "volte-mt:";

/// Outcome of feeding one inbound MT MESSAGE body into the reassembler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MtIngest {
    /// A single-part message, or the final segment that completes a multipart
    /// message: fully assembled and ready to persist.
    Complete(AssembledSms),
    /// A multipart segment was buffered; more segments are still awaited.
    Buffered { reference: u16, have: usize, total: u8 },
    /// The RP-DATA could not be parsed as a deliver.
    ParseError,
}

/// A fully assembled MT SMS ready for persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledSms {
    pub originator: String,
    pub text: String,
    pub service_center_timestamp: String,
    /// Reference of the concatenation group, or None for single-part.
    pub segment_reference: Option<u16>,
    pub segment_total: u8,
    /// Deterministic dedup marker for the shared `sms_messages.pdu` column.
    pub dedup_marker: String,
}

/// State for one in-flight concatenated (multipart) SMS.
#[derive(Debug, Clone)]
struct MultipartGroup {
    total: u8,
    /// sequence (1-based) -> segment deliver
    segments: HashMap<u8, MtSmsDeliver>,
}

/// Reassembles multipart MT SMS and computes dedup keys. Not thread-safe by
/// itself; the runtime wraps it in a mutex (mirroring the reference's
/// "MT multipart cache" guarded by a lock).
#[derive(Debug, Default)]
pub struct MtReassembler {
    groups: HashMap<u16, MultipartGroup>,
}

impl MtReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and ingest one inbound MT MESSAGE body (the RP-DATA bytes).
    pub fn ingest(&mut self, rp_data: &[u8]) -> MtIngest {
        let deliver = match parse_mt_rp_data(rp_data) {
            Ok(d) => d,
            Err(_) => return MtIngest::ParseError,
        };
        self.ingest_deliver(deliver)
    }

    /// Ingest an already-parsed deliver (split out for testability).
    pub fn ingest_deliver(&mut self, deliver: MtSmsDeliver) -> MtIngest {
        match deliver.segment_reference {
            None => MtIngest::Complete(assemble_single(&deliver)),
            Some(reference) => self.ingest_segment(reference, deliver),
        }
    }

    fn ingest_segment(&mut self, reference: u16, deliver: MtSmsDeliver) -> MtIngest {
        let total = deliver.segment_total.max(1);
        let group = self
            .groups
            .entry(reference)
            .or_insert_with(|| MultipartGroup {
                total,
                segments: HashMap::new(),
            });
        // Keep total in sync (segments should agree; trust the latest non-zero).
        if deliver.segment_total > 0 {
            group.total = deliver.segment_total;
        }
        group.segments.insert(deliver.segment_sequence, deliver);

        if group.segments.len() >= group.total as usize {
            // All segments present: assemble in sequence order.
            let group = self.groups.remove(&reference).expect("group present");
            MtIngest::Complete(assemble_multipart(reference, &group))
        } else {
            MtIngest::Buffered {
                reference,
                have: group.segments.len(),
                total: group.total,
            }
        }
    }

    /// Number of in-flight multipart groups (for diagnostics/tests).
    pub fn pending_groups(&self) -> usize {
        self.groups.len()
    }
}

fn assemble_single(deliver: &MtSmsDeliver) -> AssembledSms {
    AssembledSms {
        originator: deliver.originator.clone(),
        text: deliver.text.clone(),
        service_center_timestamp: deliver.service_center_timestamp.clone(),
        segment_reference: None,
        segment_total: 1,
        dedup_marker: single_marker(deliver),
    }
}

fn assemble_multipart(reference: u16, group: &MultipartGroup) -> AssembledSms {
    // Concatenate segment text in ascending sequence order.
    let mut sequences: Vec<&u8> = group.segments.keys().collect();
    sequences.sort();
    let mut text = String::new();
    let mut originator = String::new();
    let mut scts = String::new();
    for seq in sequences {
        let seg = &group.segments[seq];
        if originator.is_empty() {
            originator = seg.originator.clone();
        }
        if scts.is_empty() {
            scts = seg.service_center_timestamp.clone();
        }
        text.push_str(&seg.text);
    }
    let marker = multipart_marker(&originator, reference, group.total, &scts);
    AssembledSms {
        originator,
        text,
        service_center_timestamp: scts,
        segment_reference: Some(reference),
        segment_total: group.total,
        dedup_marker: marker,
    }
}

/// Dedup marker for a single-part MT SMS: stable over originator + SCTS + text
/// hash, so a retransmitted identical message maps to the same marker.
fn single_marker(deliver: &MtSmsDeliver) -> String {
    format!(
        "{}single:{}:{}:{}",
        MT_MARKER_PREFIX,
        deliver.originator,
        deliver.service_center_timestamp,
        text_hash(&deliver.text),
    )
}

/// Dedup marker for a completed multipart MT SMS: keyed on the concatenation
/// reference + total + originator (+ first-segment SCTS), matching the
/// reference's "segment:<ref>:<total>" grouping idea.
fn multipart_marker(originator: &str, reference: u16, total: u8, scts: &str) -> String {
    format!(
        "{}segment:{:04x}:{}:{}:{}",
        MT_MARKER_PREFIX, reference, total, originator, scts,
    )
}

/// Short, stable hash of message text (hex md5) for the dedup marker. Avoids
/// storing the plaintext in the marker while keeping it deterministic.
fn text_hash(text: &str) -> String {
    format!("{:x}", md5::compute(text.as_bytes()))
}

/// Build the RP-ACK RPDU body to return to the network for a received MT SMS.
/// The `reference` is the RP message reference from the inbound deliver.
pub fn build_rp_ack_body(reference: u8) -> Vec<u8> {
    crate::access::vowifi::sms::build_network_rp_ack(reference)
}

// ============================ MO (send) side ============================

pub use crate::access::vowifi::sms::MoSmsSubmission;

/// Build a single-part MO SMS submission (RP-DATA + SMS-SUBMIT TPDU). Reuses the
/// vowifi 3GPP codec. Long-message segmentation (>160 GSM7 / >70 UCS2) is a
/// future addition (the reused codec only builds single parts).
pub fn build_mo_submission(
    recipient: &str,
    text: &str,
    service_center: &str,
) -> Result<MoSmsSubmission, SmsEncodingError> {
    crate::access::vowifi::sms::build_single_part_mo_submission(recipient, text, service_center)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(
        originator: &str,
        text: &str,
        scts: &str,
        reference: Option<u16>,
        seq: u8,
        total: u8,
    ) -> MtSmsDeliver {
        MtSmsDeliver {
            rp_message_reference: 0,
            originator: originator.to_string(),
            text: text.to_string(),
            user_data_bytes: text.len(),
            service_center_timestamp: scts.to_string(),
            segment_reference: reference,
            segment_sequence: seq,
            segment_total: total,
        }
    }

    #[test]
    fn single_part_completes_immediately() {
        let mut r = MtReassembler::new();
        let out = r.ingest_deliver(deliver("+8613800138000", "hello", "0011223344556677", None, 0, 0));
        match out {
            MtIngest::Complete(sms) => {
                assert_eq!(sms.text, "hello");
                assert_eq!(sms.segment_reference, None);
                assert_eq!(sms.segment_total, 1);
                assert!(sms.dedup_marker.starts_with("volte-mt:single:"));
            }
            other => panic!("expected Complete, got {other:?}"),
        }
        assert_eq!(r.pending_groups(), 0);
    }

    #[test]
    fn multipart_buffers_then_assembles_in_order() {
        let mut r = MtReassembler::new();
        // Receive segment 2 first (out of order), then segment 1.
        let out2 = r.ingest_deliver(deliver("+861380", "World", "00", Some(0x1234), 2, 2));
        assert!(matches!(out2, MtIngest::Buffered { have: 1, total: 2, .. }));
        assert_eq!(r.pending_groups(), 1);

        let out1 = r.ingest_deliver(deliver("+861380", "Hello ", "00", Some(0x1234), 1, 2));
        match out1 {
            MtIngest::Complete(sms) => {
                // Assembled in ascending sequence order: "Hello " + "World".
                assert_eq!(sms.text, "Hello World");
                assert_eq!(sms.segment_reference, Some(0x1234));
                assert_eq!(sms.segment_total, 2);
                assert!(sms.dedup_marker.starts_with("volte-mt:segment:1234:2:"));
            }
            other => panic!("expected Complete, got {other:?}"),
        }
        assert_eq!(r.pending_groups(), 0, "group cleared after assembly");
    }

    #[test]
    fn identical_single_parts_produce_same_marker() {
        let mut r = MtReassembler::new();
        let a = match r.ingest_deliver(deliver("+86138", "dup", "AABBCC", None, 0, 0)) {
            MtIngest::Complete(s) => s,
            _ => panic!(),
        };
        let b = match r.ingest_deliver(deliver("+86138", "dup", "AABBCC", None, 0, 0)) {
            MtIngest::Complete(s) => s,
            _ => panic!(),
        };
        assert_eq!(a.dedup_marker, b.dedup_marker, "same content -> same dedup key");
    }

    #[test]
    fn different_text_produces_different_marker() {
        let mut r = MtReassembler::new();
        let a = match r.ingest_deliver(deliver("+86138", "one", "AABBCC", None, 0, 0)) {
            MtIngest::Complete(s) => s,
            _ => panic!(),
        };
        let b = match r.ingest_deliver(deliver("+86138", "two", "AABBCC", None, 0, 0)) {
            MtIngest::Complete(s) => s,
            _ => panic!(),
        };
        assert_ne!(a.dedup_marker, b.dedup_marker);
    }

    #[test]
    fn parse_error_on_garbage_body() {
        let mut r = MtReassembler::new();
        assert_eq!(r.ingest(&[0xff, 0xff, 0xff]), MtIngest::ParseError);
    }

    #[test]
    fn rp_ack_body_is_two_bytes() {
        // build_network_rp_ack returns [0x02, reference].
        assert_eq!(build_rp_ack_body(0x42), vec![0x02, 0x42]);
    }

    #[test]
    fn transport_tag_and_marker_prefix_are_stable() {
        assert_eq!(TRANSPORT_TAG, "volte_ims");
        assert_eq!(MT_MARKER_PREFIX, "volte-mt:");
    }
}
