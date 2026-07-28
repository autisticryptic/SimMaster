//! Pure SIP dialog and INVITE transaction state for the Asterisk-facing leg.
//!
//! The trunk endpoint is a small B2BUA: the Asterisk dialog and the operator
//! IMS dialog deliberately keep independent Call-IDs, tags, CSeq values and
//! transaction branches. This module owns only the Asterisk-side identifiers
//! and state transitions; network I/O stays in `driver.rs` and cross-leg
//! commands stay in `bridge.rs`.

use crate::{connectivity::core::sip_frame, services::trunk::sip};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogDirection {
    AsteriskOriginated,
    OperatorOriginated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteTransactionState {
    Proceeding,
    AcceptedAwaitingAck,
    Confirmed,
    Terminated,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SipDialog {
    pub direction: DialogDirection,
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub local_uri: String,
    pub remote_uri: String,
    #[allow(dead_code)]
    pub remote_target: String,
    #[allow(dead_code)]
    pub invite_cseq: u32,
    pub next_local_cseq: u32,
    pub state: InviteTransactionState,
    pub initial_invite: Vec<u8>,
}

impl SipDialog {
    pub fn from_asterisk_invite(frame: &[u8]) -> Result<Self, String> {
        let call_id = required_header(frame, "Call-ID")?;
        let from = required_header(frame, "From")?;
        let to = required_header(frame, "To")?;
        let remote_uri = sip_frame::uri_from_header_value(&from)
            .ok_or_else(|| "trunk_invite_from_uri_invalid".to_string())?;
        let local_uri = sip_frame::uri_from_header_value(&to)
            .ok_or_else(|| "trunk_invite_to_uri_invalid".to_string())?;
        let remote_tag =
            header_tag(&from).ok_or_else(|| "trunk_invite_from_tag_missing".to_string())?;
        let request_uri = request_uri(frame)?;
        let invite_cseq = cseq_number(frame, "INVITE")?;
        Ok(Self {
            direction: DialogDirection::AsteriskOriginated,
            call_id,
            local_tag: sip::token(8),
            remote_tag: Some(remote_tag),
            local_uri,
            remote_uri,
            remote_target: request_uri,
            invite_cseq,
            next_local_cseq: invite_cseq.saturating_add(1),
            state: InviteTransactionState::Proceeding,
            initial_invite: frame.to_vec(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)]
    pub fn for_operator_invite(
        call_id: String,
        local_tag: String,
        local_uri: String,
        remote_uri: String,
        remote_target: String,
        invite_cseq: u32,
        frame: Vec<u8>,
    ) -> Self {
        Self {
            direction: DialogDirection::OperatorOriginated,
            call_id,
            local_tag,
            remote_tag: None,
            local_uri,
            remote_uri,
            remote_target,
            invite_cseq,
            next_local_cseq: invite_cseq.saturating_add(1),
            state: InviteTransactionState::Proceeding,
            initial_invite: frame,
        }
    }

    pub fn on_provisional(&mut self, status: u16) -> Result<(), String> {
        if !(100..200).contains(&status) || self.state != InviteTransactionState::Proceeding {
            return Err("trunk_dialog_invalid_provisional_transition".to_string());
        }
        Ok(())
    }

    pub fn on_final(&mut self, status: u16) -> Result<(), String> {
        if status < 200 || self.state != InviteTransactionState::Proceeding {
            return Err("trunk_dialog_invalid_final_transition".to_string());
        }
        self.state = if status < 300 {
            InviteTransactionState::AcceptedAwaitingAck
        } else {
            InviteTransactionState::Failed
        };
        Ok(())
    }

    pub fn on_ack(&mut self) -> Result<(), String> {
        if self.state != InviteTransactionState::AcceptedAwaitingAck {
            return Err("trunk_dialog_unexpected_ack".to_string());
        }
        self.state = InviteTransactionState::Confirmed;
        Ok(())
    }

    pub fn on_cancel(&mut self) -> Result<(), String> {
        if self.state != InviteTransactionState::Proceeding {
            return Err("trunk_dialog_cancel_after_final".to_string());
        }
        self.state = InviteTransactionState::Failed;
        Ok(())
    }

    pub fn on_bye(&mut self) -> Result<(), String> {
        if self.state != InviteTransactionState::Confirmed {
            return Err("trunk_dialog_bye_before_confirmed".to_string());
        }
        self.state = InviteTransactionState::Terminated;
        Ok(())
    }

    pub fn begin_local_request(&mut self) -> Result<u32, String> {
        if self.state != InviteTransactionState::Confirmed {
            return Err("trunk_dialog_request_before_confirmed".to_string());
        }
        let cseq = self.next_local_cseq;
        self.next_local_cseq = self.next_local_cseq.saturating_add(1);
        Ok(cseq)
    }

    pub fn learn_remote_tag(&mut self, frame: &[u8]) {
        if self.remote_tag.is_none() {
            self.remote_tag = sip_frame::header_value(frame, "To")
                .as_deref()
                .and_then(header_tag);
        }
    }
}

pub fn call_id(frame: &[u8]) -> Option<String> {
    sip_frame::header_value(frame, "Call-ID")
}

pub fn cseq_number(frame: &[u8], expected_method: &str) -> Result<u32, String> {
    let value = required_header(frame, "CSeq")?;
    let mut parts = value.split_whitespace();
    let number = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| "trunk_cseq_invalid".to_string())?;
    let method = parts
        .next()
        .ok_or_else(|| "trunk_cseq_method_missing".to_string())?;
    if !method.eq_ignore_ascii_case(expected_method) {
        return Err("trunk_cseq_method_mismatch".to_string());
    }
    Ok(number)
}

pub fn request_uri(frame: &[u8]) -> Result<String, String> {
    let line_end = frame
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| "trunk_request_line_missing".to_string())?;
    let line = std::str::from_utf8(&frame[..line_end])
        .map_err(|_| "trunk_request_line_invalid".to_string())?;
    line.split_whitespace()
        .nth(1)
        .filter(|uri| uri.starts_with("sip:"))
        .map(str::to_string)
        .ok_or_else(|| "trunk_request_uri_invalid".to_string())
}

fn required_header(frame: &[u8], name: &str) -> Result<String, String> {
    sip_frame::header_value(frame, name)
        .ok_or_else(|| format!("trunk_request_{}_missing", name.to_ascii_lowercase()))
}

fn header_tag(value: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("tag")
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> &'static [u8] {
        b"INVITE sip:41000@simadmin SIP/2.0\r\nFrom: <sip:6108@pbx>;tag=remote-a\r\nTo: <sip:41000@simadmin>\r\nCall-ID: call-a\r\nCSeq: 42 INVITE\r\nContent-Length: 0\r\n\r\n"
    }

    #[test]
    fn inbound_dialog_keeps_independent_identifiers() {
        let dialog = SipDialog::from_asterisk_invite(invite()).unwrap();
        assert_eq!(dialog.call_id, "call-a");
        assert_eq!(dialog.remote_tag.as_deref(), Some("remote-a"));
        assert_eq!(dialog.invite_cseq, 42);
        assert_eq!(dialog.remote_target, "sip:41000@simadmin");
        assert!(!dialog.local_tag.is_empty());
    }

    #[test]
    fn invite_state_requires_ack_before_bye() {
        let mut dialog = SipDialog::from_asterisk_invite(invite()).unwrap();
        dialog.on_provisional(180).unwrap();
        assert!(dialog.on_bye().is_err());
        dialog.on_final(200).unwrap();
        dialog.on_ack().unwrap();
        dialog.on_bye().unwrap();
        assert_eq!(dialog.state, InviteTransactionState::Terminated);
    }

    #[test]
    fn cancel_only_applies_before_final_response() {
        let mut dialog = SipDialog::from_asterisk_invite(invite()).unwrap();
        dialog.on_cancel().unwrap();
        assert_eq!(dialog.state, InviteTransactionState::Failed);
        assert!(dialog.on_cancel().is_err());
    }
}
