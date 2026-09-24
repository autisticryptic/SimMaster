//! Unsolicited AT indications are events, not replies to unrelated commands.
//!
//! Keep only coalesced hints here: no caller numbers, USSD text, SMS bodies,
//! SIM identities or vendor payloads. Consumers reconcile authoritative state
//! under their normal ownership/admission checks before acting on a hint.

use serde::{Deserialize, Serialize};

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

#[derive(Default)]
pub(super) struct UrcRouter {
    events: UrcEvents,
    pending_pdu: bool,
}

impl UrcRouter {
    pub fn take(&mut self) -> UrcEvents {
        std::mem::take(&mut self.events)
    }

    pub fn reset_frame(&mut self) {
        self.pending_pdu = false;
    }

    /// True means the line must not enter the current command's reply.
    pub fn route(&mut self, line: &str, command: Option<&str>) -> bool {
        let line = line.trim();
        if self.pending_pdu {
            self.pending_pdu = false;
            // Only discard a well-framed PDU continuation. Never swallow OK,
            // ERROR, an echo or another indication as an assumed SMS body.
            if !line.is_empty()
                && line.len() % 2 == 0
                && line.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return true;
            }
        }
        let upper = line.to_ascii_uppercase();
        let command = command.unwrap_or("").trim().to_ascii_uppercase();
        // Query responses share names with registration and vendor URCs.
        // Keep an exact command-family reply (not a substring/prefix match).
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
                // They terminate dial/answer, but an asynchronous call ending
                // must not fail a concurrent signal/SIM/storage query.
                return !(command.starts_with("ATD") || command == "ATA");
            }
            "RING" => self.events.call_changed = true,
            _ if upper.starts_with("+CMTI:") => self.events.sms_stored = true,
            _ if upper.starts_with("+CMT:") => {
                self.events.sms_direct = true;
                self.pending_pdu = true;
            }
            _ if upper.starts_with("+CDS:") => {
                self.events.delivery_report = true;
                self.pending_pdu = true;
            }
            _ if upper.starts_with("+CDSI:") => self.events.delivery_report = true,
            _ if ["+CLIP:", "+CRING:", "+CCWA:", "+COLP:"]
                .iter()
                .any(|p| upper.starts_with(p)) =>
            {
                self.events.call_changed = true;
            }
            _ if upper.starts_with("+CUSD:") => self.events.ussd = true,
            _ if ["+CREG:", "+CGREG:", "+CEREG:", "+C5GREG:"]
                .iter()
                .any(|p| upper.starts_with(p)) =>
            {
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
        let mut router = UrcRouter::default();
        assert!(router.route("+CMT: ,4", Some("AT+CMGL=4")));
        assert!(router.route("00112233", Some("AT+CMGL=4")));
        assert!(!router.route("+CMGL: 1,0,,4", Some("AT+CMGL=4")));
        assert!(!router.route("AABBCCDD", Some("AT+CMGL=4")));
        assert!(router.route("+CDS: 4", None));
        assert!(!router.route("OK", Some("AT+CMGF=0")));
        assert!(router.take().needs_sms_scan());
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
