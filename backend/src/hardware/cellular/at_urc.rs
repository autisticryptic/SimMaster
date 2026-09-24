//! Unsolicited AT indications are events, not replies to unrelated commands.
//!
//! Public hints contain no payloads. Direct PDUs live in a separate, bounded
//! queue, bound to an explicitly observed SIM. Only durable ingestion may
//! retire its tokens; an in-memory PDU is not proof of delivery.

use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrcEvents {
    pub sms_stored: bool,
    pub sms_direct: bool,
    pub delivery_report: bool,
    pub call_changed: bool,
    pub ussd: bool,
    pub registration_changed: bool,
    pub vendor: bool,
}

impl UrcEvents {
    pub fn needs_sms_scan(&self) -> bool {
        self.sms_stored || self.sms_direct || self.delivery_report
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectKind {
    Deliver,
    StatusReport,
}

impl DirectKind {
    pub fn inbox_kind(self) -> &'static str {
        match self {
            Self::Deliver => "deliver",
            Self::StatusReport => "report",
        }
    }
}

/// Private transport response, never published on the observation bus.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectPdu {
    pub token: u64,
    pub kind: DirectKind,
    pub sim_key: String,
    pub hex: String,
}
impl std::fmt::Debug for DirectPdu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectPdu")
            .field("token", &self.token)
            .field("kind", &self.kind)
            .field("bytes", &(self.hex.len() / 2))
            .finish()
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectBatch {
    pub pdus: Vec<DirectPdu>,
    pub dropped: u64,
}

struct QueuedPdu {
    value: DirectPdu,
    received: Instant,
    ack_live: bool,
}

#[derive(Default)]
pub(super) struct UrcRouter {
    events: UrcEvents,
    pending_pdu: Option<(DirectKind, Option<usize>)>,
    direct: VecDeque<QueuedPdu>,
    sim_key: Option<String>,
    next_token: u64,
    dropped: u64,
    framing_fault: bool,
    ack_blocked: bool,
}

impl UrcRouter {
    pub fn take(&mut self) -> UrcEvents {
        std::mem::take(&mut self.events)
    }

    fn lost_pdu(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
        self.ack_blocked = true;
    }

    pub fn reset_frame(&mut self) {
        if self.pending_pdu.take().is_some() {
            self.lost_pdu();
        }
        self.framing_fault = false;
    }

    pub fn transport_reset(&mut self) {
        self.reset_frame();
        // A reopened fd cannot establish whether an old network ACK is still
        // outstanding. Retain bytes for ingestion, never replay old CNMA.
        for pdu in &mut self.direct {
            pdu.ack_live = false;
        }
    }

    pub fn clear_private_data(&mut self) {
        self.reset_frame();
        self.dropped = self.dropped.saturating_add(self.direct.len() as u64);
        self.direct.clear();
        self.sim_key = None;
        self.ack_blocked = false;
    }

    pub fn bind_sim(&mut self, sim_key: &str) -> Result<(), String> {
        if sim_key.len() != 64 || !sim_key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("AT direct SIM scope invalid".into());
        }
        if self.sim_key.as_deref() != Some(sim_key) {
            // Do not attribute pre-admission or previous-card bytes to a new
            // SIM. Stored reception remains the default/loss fallback.
            self.clear_private_data();
            self.sim_key = Some(sim_key.into());
        }
        Ok(())
    }

    pub fn direct_batch(&self) -> DirectBatch {
        DirectBatch {
            pdus: self.direct.iter().map(|p| p.value.clone()).collect(),
            dropped: self.dropped,
        }
    }

    pub fn take_direct(&mut self, token: u64, acknowledge: bool) -> Option<DirectPdu> {
        let position = self.direct.iter().position(|p| p.value.token == token)?;
        let pdu = &self.direct[position];
        // CNMA has no message ID. Reject ambiguity, loss, stale sessions and
        // old deliveries. Ten seconds is a conservative local upper bound,
        // not a claim about every firmware's acknowledgement timer.
        if acknowledge
            && (self.direct.len() != 1
                || self.pending_pdu.is_some()
                || self.ack_blocked
                || !pdu.ack_live
                || pdu.received.elapsed() > Duration::from_secs(10)
                || self.sim_key.as_deref() != Some(pdu.value.sim_key.as_str()))
        {
            return None;
        }
        self.direct.remove(position).map(|p| p.value)
    }

    pub fn framing_fault(&mut self) -> bool {
        std::mem::take(&mut self.framing_fault)
    }
    pub fn has_pending_frame(&self) -> bool {
        self.pending_pdu.is_some()
    }
    pub fn block_ack(&mut self) {
        self.ack_blocked = true;
    }

    fn begin_pdu(&mut self, kind: DirectKind, header: &str) {
        if self.pending_pdu.take().is_some() {
            self.lost_pdu();
        }
        let length = header
            .rsplit(',')
            .next()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| (1..=255).contains(n));
        // Even an invalid header owns its following PDU line. Never leak it
        // into a concurrent +CMGL reply as somebody else's storage content.
        self.pending_pdu = Some((kind, length));
    }

    /// True means the line must not enter the current command's reply.
    pub fn route(&mut self, line: &str, command: Option<&str>) -> bool {
        let line = line.trim();
        if self.pending_pdu.is_some() && line.is_empty() {
            return true;
        }
        if let Some((kind, length)) = self.pending_pdu.take() {
            if !line.is_empty()
                && line.len() % 2 == 0
                && line.bytes().all(|b| b.is_ascii_hexdigit())
            {
                let smsc_length = line
                    .get(..2)
                    .and_then(|v| u8::from_str_radix(v, 16).ok())
                    .map(usize::from);
                let tpdu = smsc_length.and_then(|n| line.get((n + 1) * 2..));
                let mti = tpdu
                    .and_then(|s| s.get(..2))
                    .and_then(|v| u8::from_str_radix(v, 16).ok())
                    .map(|v| v & 3);
                let valid = line.len() <= 1024
                    && smsc_length
                        .zip(length)
                        .is_some_and(|(n, l)| line.len() / 2 == n + 1 + l)
                    && mti == Some(if kind == DirectKind::Deliver { 0 } else { 2 });
                if valid && self.direct.len() < 16 && self.sim_key.is_some() {
                    self.next_token = self.next_token.wrapping_add(1).max(1);
                    self.direct.push_back(QueuedPdu {
                        value: DirectPdu {
                            token: self.next_token,
                            kind,
                            sim_key: self.sim_key.clone().unwrap(),
                            hex: line.to_ascii_uppercase(),
                        },
                        received: Instant::now(),
                        ack_live: true,
                    });
                } else {
                    self.lost_pdu();
                    self.framing_fault |= !valid && command.is_some();
                }
                return true;
            }
            self.lost_pdu();
            self.framing_fault = command.is_some();
        }
        let upper = line.to_ascii_uppercase();
        let command = command.unwrap_or("").trim().to_ascii_uppercase();
        // Query responses share names with registration/vendor URCs.
        if let Some((prefix, _)) = upper.split_once(':') {
            if let Some(stem) = command.strip_prefix("AT") {
                let stem = stem.split(['?', '=']).next().unwrap_or(stem);
                if stem == prefix {
                    return false;
                }
            }
        }
        match upper.as_str() {
            "NO CARRIER" | "BUSY" | "NO ANSWER" | "NO DIALTONE" | "NO DIAL TONE" => {
                self.events.call_changed = true;
                return !(command.starts_with("ATD") || command == "ATA");
            }
            "RING" => self.events.call_changed = true,
            _ if upper.starts_with("+CMTI:") => self.events.sms_stored = true,
            _ if upper.starts_with("+CMT:") => {
                self.events.sms_direct = true;
                self.begin_pdu(
                    DirectKind::Deliver,
                    line.split_once(':').map(|(_, v)| v).unwrap_or(""),
                );
            }
            _ if upper.starts_with("+CDS:") => {
                self.events.delivery_report = true;
                self.begin_pdu(
                    DirectKind::StatusReport,
                    line.split_once(':').map(|(_, v)| v).unwrap_or(""),
                );
            }
            _ if upper.starts_with("+CDSI:") => self.events.delivery_report = true,
            _ if ["+CLIP:", "+CRING:", "+CCWA:", "+COLP:"]
                .iter()
                .any(|p| upper.starts_with(p)) =>
            {
                self.events.call_changed = true
            }
            _ if upper.starts_with("+CUSD:") => self.events.ussd = true,
            _ if ["+CREG:", "+CGREG:", "+CEREG:", "+C5GREG:"]
                .iter()
                .any(|p| upper.starts_with(p)) =>
            {
                self.events.registration_changed = true
            }
            _ if upper.starts_with("+CPIN:") => {
                self.clear_private_data();
                self.events.registration_changed = true;
            }
            _ if upper.starts_with("+QIND:") => self.events.vendor = true,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bound() -> UrcRouter {
        let mut router = UrcRouter::default();
        router.bind_sim(&"a".repeat(64)).unwrap();
        router
    }
    fn deliver(router: &mut UrcRouter) {
        assert!(router.route("+CMT: ,4", None));
        assert!(router.route("0000112233", None));
    }
    #[test]
    fn query_response_belongs_to_its_command_but_other_urcs_do_not() {
        let mut router = UrcRouter::default();
        assert!(!router.route("+CEREG: 2,5", Some("AT+CEREG?")));
        assert!(router.route("+CEREG: 5", Some("AT+CSQ")));
        assert!(router.route("+CMTI: \"SM\",1", Some("AT+CIMI")));
        assert!(!router.route("+CUSD: 0,\"done\",15", Some("AT+CUSD=1,\"*123#\",15")));
        assert!(router.route("+CUSD: 0,\"late\",15", Some("AT+CSQ")));
        let events = router.take();
        assert!(events.registration_changed && events.sms_stored && events.ussd);
        assert_eq!(router.take(), UrcEvents::default());
    }
    #[test]
    fn call_finals_terminate_only_the_call_transaction() {
        for value in ["NO CARRIER", "BUSY", "NO ANSWER", "NO DIALTONE"] {
            let mut router = UrcRouter::default();
            assert!(router.route(value, Some("AT+CSQ")));
            assert!(!router.route(value, Some("ATD123;")));
            assert!(!router.route(value, Some("ATA")));
            assert!(router.take().call_changed);
        }
    }
    #[test]
    fn direct_sms_and_reports_cannot_become_stored_message_payloads() {
        let mut router = bound();
        assert!(router.route("+CMT: ,4", Some("AT+CMGL=4")));
        assert!(router.route("0000112233", Some("AT+CMGL=4")));
        assert!(!router.route("+CMGL: 1,0,,4", Some("AT+CMGL=4")));
        assert!(!router.route("AABBCCDD", Some("AT+CMGL=4")));
        assert!(router.route("+CDS: 4", None));
        assert!(!router.route("OK", Some("AT+CMGF=0")));
        assert!(router.framing_fault());
        assert!(router.take().needs_sms_scan());
    }
    #[test]
    fn pre_admission_and_previous_sim_pdus_are_not_relabelled() {
        let mut router = UrcRouter::default();
        deliver(&mut router);
        assert!(router.direct_batch().pdus.is_empty());
        router.bind_sim(&"a".repeat(64)).unwrap();
        deliver(&mut router);
        router.bind_sim(&"a".repeat(64)).unwrap();
        assert_eq!(router.direct_batch().pdus.len(), 1);
        router.bind_sim(&"b".repeat(64)).unwrap();
        assert!(router.direct_batch().pdus.is_empty());
        deliver(&mut router);
        router.route("+CPIN: NOT READY", None);
        assert!(router.direct_batch().pdus.is_empty());
    }
    #[test]
    fn ack_rejects_multiple_partial_stale_lost_and_reopened_deliveries() {
        let mut router = bound();
        deliver(&mut router);
        let token = router.direct_batch().pdus[0].token;
        router.transport_reset();
        assert!(router.take_direct(token, true).is_none());
        assert!(router.take_direct(token, false).is_some());
        deliver(&mut router);
        let token = router.direct_batch().pdus[0].token;
        router.direct[0].received = Instant::now() - Duration::from_secs(11);
        assert!(router.take_direct(token, true).is_none());
        deliver(&mut router);
        assert!(router.take_direct(token, true).is_none());
        let mut router = bound();
        deliver(&mut router);
        let token = router.direct_batch().pdus[0].token;
        router.route("+CMT: ,4", None);
        assert!(router.take_direct(token, true).is_none());
        router.route("000011", None);
        assert!(router.take_direct(token, true).is_none());
    }
    #[test]
    fn direct_queue_is_bounded_and_retires_tokens_only_once() {
        let mut router = bound();
        deliver(&mut router);
        let pdu = router.direct_batch().pdus[0].clone();
        assert!(!format!("{pdu:?}").contains(&pdu.hex));
        assert!(router.take_direct(pdu.token, true).is_some());
        assert!(router.take_direct(pdu.token, true).is_none());
        for _ in 0..100 {
            deliver(&mut router);
        }
        assert_eq!(router.direct_batch().pdus.len(), 16);
        assert_eq!(router.direct_batch().dropped, 84);
    }
    #[test]
    fn coalesced_events_never_retain_sensitive_payloads_or_grow_with_floods() {
        let mut router = UrcRouter::default();
        for _ in 0..10000 {
            assert!(router.route("+CLIP: \"private-number\",129", None));
            assert!(router.route("+QIND: private-vendor-data", None));
        }
        let encoded = serde_json::to_string(&router.take()).unwrap();
        assert!(!encoded.contains("private"));
        assert!(encoded.len() < 256);
    }
}
