//! Replay the private native inbox into application SMS records. No hardware
//! access here: every part is already durable before CNMA/storage cleanup.
use crate::{
    connectivity::core::sms_codec::{self, ModemDeliver},
    hardware::cellular::backends::direct_sms::{digest, recipient_key, unhex},
    platform::db::{
        native_sms_inbox::{InboxMessage, NativeReport},
        Database, SmsMessage,
    },
    services::orchestrator::{message_fingerprint, MessageFingerprintInput},
};
use std::collections::BTreeMap;

struct Part {
    id: i64,
    hex: String,
    decoded: ModemDeliver,
    scts: i64,
}
const ASSEMBLY_WINDOW_SECONDS: i64 = 300;

/// SIM scope comes from the admitted capture, never a current-line guess for
/// bytes left by a previous SIM. Unscoped legacy claims cannot discard data.
pub fn consume_pending(
    db: &Database,
    line: &str,
    sim: &str,
    dedupe: bool,
) -> rusqlite::Result<Vec<SmsMessage>> {
    let mut groups: BTreeMap<(String, String, u16, u8), Vec<Part>> = BTreeMap::new();
    let mut complete = Vec::new();
    for row in db.pending_native_pdus(line, sim)? {
        let Some(bytes) = unhex(&row.hex) else {
            db.quarantine_native_pdu(row.id, line, sim)?;
            continue;
        };
        if row.kind == "report" {
            if let Ok(report) = sms_codec::parse_modem_status_report(&bytes) {
                if let Ok(recipient) = recipient_key(&report.recipient) {
                    // Unknown/ambiguous reports remain pending. No native send
                    // is 'delivered' merely because a CDS hint was observed.
                    let _ = db.consume_native_report(NativeReport {
                        id: row.id,
                        line_id: line,
                        sim_key: sim,
                        recipient_key: &recipient,
                        reference: report.reference,
                        scts: &report.service_center_timestamp,
                        status: report.status,
                    })?;
                    continue;
                }
            }
            db.quarantine_native_pdu(row.id, line, sim)?;
            continue;
        }
        let Ok(decoded) = sms_codec::parse_modem_deliver(&bytes) else {
            db.quarantine_native_pdu(row.id, line, sim)?;
            continue;
        };
        let Ok(stamp) =
            chrono::DateTime::parse_from_rfc3339(&decoded.message.service_center_timestamp)
        else {
            db.quarantine_native_pdu(row.id, line, sim)?;
            continue;
        };
        let part = Part {
            id: row.id,
            hex: row.hex,
            decoded,
            scts: stamp.timestamp(),
        };
        let message = &part.decoded.message;
        if message.segment_total == 1 {
            complete.push(vec![part]);
        } else if let Some(reference) = message.segment_reference {
            groups
                .entry((
                    message.originator.clone(),
                    part.decoded.assembly_key.clone(),
                    reference,
                    message.segment_total,
                ))
                .or_default()
                .push(part);
        }
    }
    for (_, mut parts) in groups {
        parts.sort_by_key(|p| (p.scts, p.id));
        let mut window = Vec::new();
        for part in parts {
            if window
                .first()
                .is_some_and(|first: &Part| part.scts - first.scts > ASSEMBLY_WINDOW_SECONDS)
            {
                if let Some(group) = assemble(std::mem::take(&mut window)) {
                    complete.push(group);
                }
            }
            window.push(part);
        }
        if let Some(group) = assemble(window) {
            complete.push(group);
        }
    }
    let mut messages = Vec::new();
    for mut parts in complete {
        parts.sort_by_key(|p| (p.decoded.message.segment_sequence, p.id));
        let first = &parts[0].decoded.message;
        let mut last = None;
        let text = parts
            .iter()
            .filter_map(|p| {
                let sequence = p.decoded.message.segment_sequence;
                if last == Some(sequence) {
                    None
                } else {
                    last = Some(sequence);
                    Some(p.decoded.message.text.as_str())
                }
            })
            .collect::<String>();
        let fingerprint = message_fingerprint(&MessageFingerprintInput {
            service_center_timestamp: &first.service_center_timestamp,
            originator: &first.originator,
            text: &text,
            segment_reference: None,
            segment_sequence: 1,
            segment_total: 1,
        });
        let material = parts
            .iter()
            .map(|p| p.hex.as_str())
            .collect::<Vec<_>>()
            .join(":");
        let marker = format!(
            "nativefp:{}",
            digest(format!("{sim}:{material}").as_bytes())
        );
        let ids = parts.iter().map(|p| p.id).collect::<Vec<_>>();
        if let Some(sms) = db.promote_native_sms(InboxMessage {
            ids: &ids,
            line_id: line,
            sim_key: sim,
            number: &first.originator,
            text: &text,
            timestamp: &first.service_center_timestamp,
            marker: &marker,
            fingerprint: dedupe.then_some(fingerprint.as_str()),
        })? {
            messages.push(sms);
        }
    }
    Ok(messages)
}

fn assemble(parts: Vec<Part>) -> Option<Vec<Part>> {
    let total = parts.first()?.decoded.message.segment_total;
    let mut sequences = BTreeMap::new();
    if parts.len() > 255 {
        return None;
    }
    for part in &parts {
        let message = &part.decoded.message;
        if let Some(previous) = sequences.insert(message.segment_sequence, message) {
            // An identical retransmission is not a conflict. Different text,
            // SCTS or segment metadata sharing a reference is ambiguous.
            if !previous.is_duplicate_delivery(message) {
                return None;
            }
        }
    }
    (sequences.len() == usize::from(total) && (1..=total).all(|n| sequences.contains_key(&n)))
        .then_some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(text: &str, reference: Option<(u8, u8, u8)>, minute: u8) -> Vec<u8> {
        let mut pdu = vec![
            0,
            if reference.is_some() { 0x40 } else { 0 },
            4,
            0x81,
            0x21,
            0x43,
            0,
            8,
            0x62,
            0x10,
            0x10,
            0,
            minute,
            0,
            0,
        ];
        let mut data = Vec::new();
        if let Some((reference, total, sequence)) = reference {
            data.extend_from_slice(&[5, 0, 3, reference, total, sequence]);
        }
        for unit in text.encode_utf16() {
            data.extend_from_slice(&unit.to_be_bytes());
        }
        pdu.push(data.len() as u8);
        pdu.extend(data);
        pdu
    }
    fn stage(db: &Database, pdu: &[u8]) {
        let hex = pdu.iter().map(|b| format!("{b:02X}")).collect::<String>();
        db.stage_native_pdu(
            "line",
            &"a".repeat(64),
            "deliver",
            &digest(hex.as_bytes()),
            &hex,
        )
        .unwrap();
    }
    fn consume(db: &Database) -> Vec<SmsMessage> {
        consume_pending(db, "line", &"a".repeat(64), true).unwrap()
    }
    #[test]
    fn multipart_out_of_order_replays_without_waiting_to_ack_all_parts() {
        let db = Database::new(":memory:".into()).unwrap();
        stage(&db, &fixture("B", Some((7, 2, 2)), 0));
        assert!(consume(&db).is_empty());
        stage(&db, &fixture("A", Some((7, 2, 1)), 0));
        let messages = consume(&db);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "AB");
        stage(&db, &fixture("A", Some((7, 2, 1)), 0));
        assert!(consume(&db).is_empty());
    }
    #[test]
    fn reused_reference_outside_window_and_conflicting_parts_do_not_merge() {
        let db = Database::new(":memory:".into()).unwrap();
        stage(&db, &fixture("old", Some((7, 2, 1)), 0));
        stage(&db, &fixture("new", Some((7, 2, 2)), 0x01)); // ten minutes, swapped BCD
        assert!(consume(&db).is_empty());
        stage(&db, &fixture("conflict", Some((7, 2, 1)), 0));
        stage(&db, &fixture("second", Some((7, 2, 2)), 0));
        assert!(consume(&db).is_empty());
    }
    #[test]
    fn truncated_payload_and_invalid_concat_are_quarantined_not_displayed() {
        let db = Database::new(":memory:".into()).unwrap();
        let mut truncated = fixture("text", None, 0);
        truncated.pop();
        stage(&db, &truncated);
        stage(&db, &fixture("bad", Some((7, 2, 0)), 0));
        assert!(consume(&db).is_empty());
        assert!(db
            .pending_native_pdus("line", &"a".repeat(64))
            .unwrap()
            .is_empty());
    }
    #[test]
    fn complete_native_message_and_its_retransmission_are_idempotent() {
        let db = Database::new(":memory:".into()).unwrap();
        let pdu = fixture("fixture", None, 0);
        stage(&db, &pdu);
        assert_eq!(consume(&db)[0].content, "fixture");
        stage(&db, &pdu);
        assert!(consume(&db).is_empty());
    }
}
