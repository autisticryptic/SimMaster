//! Asterisk ↔ operator IMS control-plane bridge.
//!
//! This is intentionally an event-driven B2BUA seam. An Asterisk INVITE is
//! parsed and acknowledged immediately, then translated into an
//! [`OperatorCommand`]. A future IMS live loop feeds the resulting
//! [`OperatorEvent`] back into the bridge, which emits the corresponding SIP
//! response or in-dialog request. No task waits synchronously for the modem.
//!
//! The D5 implementation has no live IMS voice session yet. `driver.rs` uses
//! [`OperatorAvailability::Unavailable`] and therefore returns an honest 480
//! after the initial 100 Trying. Offline tests use the same bridge with a
//! mock event source to cover 180/200/ACK/BYE/CANCEL and codec/media errors.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
};

use crate::{
    access::volte::vilte::{parse_video_sdp, VideoMediaDescription},
    ims::{
        sip_frame,
        sip_message::SipHeader,
        voice::{parse_audio_sdp, SdpAudioDescription},
    },
    trunk::{
        dialog::{self, InviteTransactionState, SipDialog},
        sip::{self, DialogRequest},
    },
};

#[allow(dead_code)] // EventDriven is enabled when the IMS live adapter is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorAvailability {
    Unavailable,
    EventDriven,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaOffer {
    pub audio: SdpAudioDescription,
    pub audio_endpoint: SocketAddr,
    pub video: Option<VideoOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoOffer {
    pub description: VideoMediaDescription,
    pub endpoint: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorCommand {
    StartCall {
        call_id: String,
        caller: String,
        callee: String,
        offer: MediaOffer,
    },
    CancelCall {
        call_id: String,
    },
    HangupCall {
        call_id: String,
    },
    Renegotiate {
        call_id: String,
        offer: MediaOffer,
    },
    ReportProvisional {
        call_id: String,
        status: u16,
        body: Option<Vec<u8>>,
    },
    AcceptCall {
        call_id: String,
        body: Vec<u8>,
    },
    RejectCall {
        call_id: String,
        status: u16,
    },
}

#[allow(dead_code)] // Constructed by the future IMS live event dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorEvent {
    Provisional {
        call_id: String,
        status: u16,
        body: Option<Vec<u8>>,
    },
    Answered {
        call_id: String,
        body: Vec<u8>,
    },
    Rejected {
        call_id: String,
        status: u16,
    },
    Unavailable {
        call_id: String,
    },
    Ended {
        call_id: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeOutput {
    pub asterisk_frames: Vec<Vec<u8>>,
    pub operator_commands: Vec<OperatorCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    MalformedRequest(String),
    InvalidState(String),
    UnsupportedMedia(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedRequest(reason) => write!(f, "malformed trunk request: {reason}"),
            Self::InvalidState(reason) => write!(f, "invalid trunk bridge state: {reason}"),
            Self::UnsupportedMedia(reason) => write!(f, "unsupported trunk media: {reason}"),
        }
    }
}

impl std::error::Error for BridgeError {}

#[derive(Debug, Clone)]
struct BridgedCall {
    dialog: SipDialog,
    operator_call_id: String,
}

#[derive(Debug, Clone)]
pub struct TrunkBridge {
    local_addr: SocketAddr,
    local_aor: String,
    asterisk_target: Option<String>,
    operator: OperatorAvailability,
    calls: HashMap<String, BridgedCall>,
}

impl TrunkBridge {
    pub fn new(local_addr: SocketAddr, local_aor: impl Into<String>) -> Self {
        Self {
            local_addr,
            local_aor: local_aor.into(),
            asterisk_target: None,
            operator: OperatorAvailability::Unavailable,
            calls: HashMap::new(),
        }
    }

    pub fn with_operator(mut self, operator: OperatorAvailability) -> Self {
        self.operator = operator;
        self
    }

    pub fn with_asterisk_target(mut self, target: impl Into<String>) -> Self {
        self.asterisk_target = Some(target.into());
        self
    }

    /// Start a mobile-terminated call toward the configured Asterisk target.
    /// The returned frame is a normal UAC INVITE; subsequent Asterisk
    /// responses are fed through [`handle_asterisk`].
    #[allow(dead_code)]
    pub fn start_operator_incoming(
        &mut self,
        operator_call_id: impl Into<String>,
        caller_uri: &str,
        body: &[u8],
    ) -> Result<BridgeOutput, BridgeError> {
        let target = self
            .asterisk_target
            .clone()
            .ok_or_else(|| BridgeError::InvalidState("asterisk_target_missing".into()))?;
        let _offer = parse_media_offer(body)?;
        let operator_call_id = operator_call_id.into();
        let call_id = format!("{}@simadmin", sip::token(12));
        let local_tag = sip::token(8);
        let cseq = 1;
        let frame = sip::build_dialog_request(&DialogRequest {
            method: "INVITE",
            request_uri: &target,
            local_addr: self.local_addr,
            from_uri: caller_uri,
            from_tag: &local_tag,
            to_uri: &target,
            to_tag: None,
            call_id: &call_id,
            cseq,
            contact_uri: Some(&self.local_aor),
            body,
        })
        .map_err(BridgeError::MalformedRequest)?;
        let outbound_frame = frame.clone();
        let dialog = SipDialog::for_operator_invite(
            call_id.clone(),
            local_tag,
            caller_uri.to_string(),
            target.clone(),
            target,
            cseq,
            frame,
        );
        self.calls.insert(
            call_id,
            BridgedCall {
                dialog,
                operator_call_id,
            },
        );
        Ok(BridgeOutput {
            asterisk_frames: vec![outbound_frame],
            ..BridgeOutput::default()
        })
    }

    #[allow(dead_code)]
    pub fn active_call_count(&self) -> usize {
        self.calls.len()
    }

    pub fn handle_asterisk(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        if !sip::is_request(frame) {
            return Ok(self.handle_asterisk_response(frame));
        }
        let method = first_token(frame).unwrap_or_default();
        match method.as_str() {
            "INVITE" => self.handle_invite(frame),
            "ACK" => self.handle_ack(frame),
            "CANCEL" => self.handle_cancel(frame),
            "BYE" => self.handle_bye(frame),
            "OPTIONS" => Ok(BridgeOutput {
                asterisk_frames: vec![
                    sip::build_response(frame, 200, "OK").map_err(BridgeError::MalformedRequest)?
                ],
                ..BridgeOutput::default()
            }),
            _ => Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response(frame, 405, "Method Not Allowed")
                    .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            }),
        }
    }

    pub fn handle_operator_event(
        &mut self,
        event: OperatorEvent,
    ) -> Result<BridgeOutput, BridgeError> {
        let call_id = match &event {
            OperatorEvent::Provisional { call_id, .. }
            | OperatorEvent::Answered { call_id, .. }
            | OperatorEvent::Rejected { call_id, .. }
            | OperatorEvent::Unavailable { call_id }
            | OperatorEvent::Ended { call_id } => call_id,
        }
        .clone();
        let asterisk_call_id = self
            .calls
            .iter()
            .find(|(_, call)| call.operator_call_id == call_id)
            .map(|(asterisk_call_id, _)| asterisk_call_id.clone())
            .ok_or_else(|| BridgeError::InvalidState("operator_call_unknown".to_string()))?;
        let Some(call) = self.calls.get_mut(&asterisk_call_id) else {
            return Err(BridgeError::InvalidState(
                "operator_call_unknown".to_string(),
            ));
        };
        let mut output = BridgeOutput::default();
        match event {
            OperatorEvent::Provisional { status, body, .. } => {
                call.dialog
                    .on_provisional(status)
                    .map_err(BridgeError::InvalidState)?;
                let body = body.unwrap_or_default();
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &call.dialog.initial_invite,
                        status,
                        reason(status),
                        Some(&call.dialog.local_tag),
                        &[],
                        &body,
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::Answered { body, .. } => {
                call.dialog
                    .on_final(200)
                    .map_err(BridgeError::InvalidState)?;
                let contact = SipHeader::new("Contact", format!("<{}>", self.local_aor));
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &call.dialog.initial_invite,
                        200,
                        "OK",
                        Some(&call.dialog.local_tag),
                        &[contact],
                        &body,
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::Rejected { status, .. } => {
                call.dialog
                    .on_final(status)
                    .map_err(BridgeError::InvalidState)?;
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &call.dialog.initial_invite,
                        status,
                        reason(status),
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::Unavailable { .. } => {
                call.dialog
                    .on_final(480)
                    .map_err(BridgeError::InvalidState)?;
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &call.dialog.initial_invite,
                        480,
                        "Temporarily Unavailable",
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::Ended { .. } => {
                if call.dialog.state == InviteTransactionState::Confirmed {
                    let cseq = call
                        .dialog
                        .begin_local_request()
                        .map_err(BridgeError::InvalidState)?;
                    let bye = sip::build_dialog_request(&DialogRequest {
                        method: "BYE",
                        request_uri: &call.dialog.remote_uri,
                        local_addr: self.local_addr,
                        from_uri: &call.dialog.local_uri,
                        from_tag: &call.dialog.local_tag,
                        to_uri: &call.dialog.remote_uri,
                        to_tag: call.dialog.remote_tag.as_deref(),
                        call_id: &call.dialog.call_id,
                        cseq,
                        contact_uri: None,
                        body: &[],
                    })
                    .map_err(BridgeError::MalformedRequest)?;
                    output.asterisk_frames.push(bye);
                    call.dialog.state = InviteTransactionState::Terminated;
                }
            }
        }
        if matches!(
            call.dialog.state,
            InviteTransactionState::Failed | InviteTransactionState::Terminated
        ) {
            self.calls.remove(&asterisk_call_id);
        }
        Ok(output)
    }

    fn handle_invite(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_invite_call-id_missing".into()))?;
        if let Some(call) = self.calls.get_mut(&call_id) {
            if call.dialog.state != InviteTransactionState::Confirmed {
                return Err(BridgeError::InvalidState(
                    "trunk_reinvite_before_confirmed".into(),
                ));
            }
            let offer = parse_offer(frame)?;
            let cseq =
                dialog::cseq_number(frame, "INVITE").map_err(BridgeError::MalformedRequest)?;
            call.dialog.next_local_cseq = cseq.saturating_add(1);
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response_with_body(
                    frame,
                    100,
                    "Trying",
                    Some(&call.dialog.local_tag),
                    &[],
                    &[],
                )
                .map_err(BridgeError::MalformedRequest)?],
                operator_commands: vec![OperatorCommand::Renegotiate { call_id, offer }],
            });
        }

        let offer = parse_offer(frame)?;
        let dialog =
            SipDialog::from_asterisk_invite(frame).map_err(BridgeError::MalformedRequest)?;
        let caller = sip_frame::header_uri(frame, "From").unwrap_or_else(|| "sip:unknown".into());
        let callee = sip_frame::header_uri(frame, "To").unwrap_or_else(|| self.local_aor.clone());
        let command = OperatorCommand::StartCall {
            call_id: call_id.clone(),
            caller,
            callee,
            offer: offer.clone(),
        };
        let mut output = BridgeOutput {
            asterisk_frames: vec![sip::build_response_with_body(
                frame,
                100,
                "Trying",
                Some(&dialog.local_tag),
                &[],
                &[],
            )
            .map_err(BridgeError::MalformedRequest)?],
            operator_commands: vec![command],
        };
        self.calls.insert(
            call_id.clone(),
            BridgedCall {
                dialog,
                operator_call_id: call_id.clone(),
            },
        );
        if self.operator == OperatorAvailability::Unavailable {
            let unavailable = self.handle_operator_event(OperatorEvent::Unavailable { call_id })?;
            output.asterisk_frames.extend(unavailable.asterisk_frames);
        }
        Ok(output)
    }

    fn handle_ack(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_ack_call-id_missing".into()))?;
        if let Some(call) = self.calls.get_mut(&call_id) {
            if call.dialog.state == InviteTransactionState::AcceptedAwaitingAck {
                call.dialog.on_ack().map_err(BridgeError::InvalidState)?;
            }
        }
        Ok(BridgeOutput::default())
    }

    fn handle_cancel(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_cancel_call-id_missing".into()))?;
        let Some(call) = self.calls.get_mut(&call_id) else {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response(
                    frame,
                    481,
                    "Call/Transaction Does Not Exist",
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        };
        call.dialog.on_cancel().map_err(BridgeError::InvalidState)?;
        let response =
            sip::build_response(frame, 200, "OK").map_err(BridgeError::MalformedRequest)?;
        let final_response = sip::build_response_with_body(
            &call.dialog.initial_invite,
            487,
            "Request Terminated",
            Some(&call.dialog.local_tag),
            &[],
            &[],
        )
        .map_err(BridgeError::MalformedRequest)?;
        self.calls.remove(&call_id);
        Ok(BridgeOutput {
            asterisk_frames: vec![response, final_response],
            operator_commands: vec![OperatorCommand::CancelCall { call_id }],
        })
    }

    fn handle_bye(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_bye_call-id_missing".into()))?;
        let Some(call) = self.calls.get_mut(&call_id) else {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response(
                    frame,
                    481,
                    "Call/Transaction Does Not Exist",
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        };
        call.dialog.on_bye().map_err(BridgeError::InvalidState)?;
        let operator_call_id = call.operator_call_id.clone();
        self.calls.remove(&call_id);
        Ok(BridgeOutput {
            asterisk_frames: vec![
                sip::build_response(frame, 200, "OK").map_err(BridgeError::MalformedRequest)?
            ],
            operator_commands: vec![OperatorCommand::HangupCall {
                call_id: operator_call_id,
            }],
        })
    }

    fn handle_asterisk_response(&mut self, frame: &[u8]) -> BridgeOutput {
        let Some(call_id) = dialog::call_id(frame) else {
            return BridgeOutput::default();
        };
        let Some(call) = self.calls.get_mut(&call_id) else {
            return BridgeOutput::default();
        };
        let status = sip::status(frame).unwrap_or(0);
        let method = sip_frame::header_value(frame, "CSeq")
            .and_then(|value| value.split_whitespace().nth(1).map(str::to_string));
        if method.as_deref() != Some("INVITE") {
            return BridgeOutput::default();
        }
        if call.dialog.direction != dialog::DialogDirection::OperatorOriginated {
            return BridgeOutput::default();
        }
        if (100..200).contains(&status) {
            let _ = call.dialog.on_provisional(status);
            return BridgeOutput {
                operator_commands: vec![OperatorCommand::ReportProvisional {
                    call_id: call.operator_call_id.clone(),
                    status,
                    body: if sip_frame::body(frame).is_empty() {
                        None
                    } else {
                        Some(sip_frame::body(frame).to_vec())
                    },
                }],
                ..BridgeOutput::default()
            };
        }
        if (200..300).contains(&status) {
            let _ = call.dialog.on_final(status);
            call.dialog.learn_remote_tag(frame);
            let ack = sip::build_ack_for_final(&call.dialog.initial_invite, frame).ok();
            let output = BridgeOutput {
                asterisk_frames: ack.into_iter().collect(),
                operator_commands: vec![OperatorCommand::AcceptCall {
                    call_id: call.operator_call_id.clone(),
                    body: sip_frame::body(frame).to_vec(),
                }],
            };
            call.dialog.state = InviteTransactionState::Confirmed;
            return output;
        }
        call.dialog.state = InviteTransactionState::Failed;
        BridgeOutput {
            operator_commands: vec![OperatorCommand::RejectCall {
                call_id: call.operator_call_id.clone(),
                status,
            }],
            ..BridgeOutput::default()
        }
    }
}

fn parse_offer(frame: &[u8]) -> Result<MediaOffer, BridgeError> {
    parse_media_offer(sip_frame::body(frame))
}

fn parse_media_offer(body: &[u8]) -> Result<MediaOffer, BridgeError> {
    let audio =
        parse_audio_sdp(body).map_err(|error| BridgeError::UnsupportedMedia(error.to_string()))?;
    let audio_endpoint = media_endpoint(&audio.connection_addr, audio.media_port)?;
    let video = parse_video_sdp(body)
        .ok()
        .map(|description| {
            let endpoint = media_endpoint(&audio.connection_addr, description.media_port)
                .map_err(|error| BridgeError::UnsupportedMedia(error.to_string()))?;
            Ok(VideoOffer {
                description,
                endpoint,
            })
        })
        .transpose()?;
    Ok(MediaOffer {
        audio,
        audio_endpoint,
        video,
    })
}

fn media_endpoint(address: &str, port: u16) -> Result<SocketAddr, BridgeError> {
    if port == 0 {
        return Err(BridgeError::UnsupportedMedia("media_port_zero".into()));
    }
    let ip = address
        .parse::<IpAddr>()
        .map_err(|_| BridgeError::UnsupportedMedia("media_address_not_ip".into()))?;
    Ok(SocketAddr::new(ip, port))
}

fn first_token(frame: &[u8]) -> Option<String> {
    frame
        .split(|byte| *byte == b' ')
        .next()
        .and_then(|token| std::str::from_utf8(token).ok())
        .map(str::to_string)
}

fn reason(status: u16) -> &'static str {
    match status {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        408 => "Request Timeout",
        480 => "Temporarily Unavailable",
        481 => "Call/Transaction Does Not Exist",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        503 => "Service Unavailable",
        _ => "Failure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    fn sdp() -> &'static [u8] {
        b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=call\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n"
    }

    fn invite() -> Vec<u8> {
        let mut frame = b"INVITE sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bK1\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>\r\nCall-ID: call-a\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: 0\r\n\r\n".to_vec();
        let marker = b"Content-Length: 0";
        let pos = frame
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap();
        frame.splice(
            pos..pos + marker.len(),
            format!("Content-Length: {}", sdp().len()).bytes(),
        );
        frame.extend_from_slice(sdp());
        frame
    }

    #[test]
    fn unavailable_operator_returns_trying_then_480() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        );
        let output = bridge.handle_asterisk(&invite()).unwrap();
        assert_eq!(output.asterisk_frames.len(), 2);
        assert!(String::from_utf8_lossy(&output.asterisk_frames[0]).starts_with("SIP/2.0 100"));
        assert!(String::from_utf8_lossy(&output.asterisk_frames[1]).starts_with("SIP/2.0 480"));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn event_driven_call_covers_answer_ack_bye() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        let output = bridge.handle_asterisk(&invite()).unwrap();
        assert_eq!(output.asterisk_frames.len(), 1);
        assert_eq!(output.operator_commands.len(), 1);
        let answered = bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        assert!(String::from_utf8_lossy(&answered.asterisk_frames[0]).starts_with("SIP/2.0 200"));
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let bye = b"BYE sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKbye\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n";
        let output = bridge.handle_asterisk(bye).unwrap();
        assert!(String::from_utf8_lossy(&output.asterisk_frames[0]).starts_with("SIP/2.0 200"));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn cancel_returns_200_and_487_and_operator_cancel() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        let cancel = b"CANCEL sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bK1\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>\r\nCall-ID: call-a\r\nCSeq: 1 CANCEL\r\nContent-Length: 0\r\n\r\n";
        let output = bridge.handle_asterisk(cancel).unwrap();
        assert!(String::from_utf8_lossy(&output.asterisk_frames[0]).starts_with("SIP/2.0 200"));
        assert!(String::from_utf8_lossy(&output.asterisk_frames[1]).starts_with("SIP/2.0 487"));
        assert_eq!(
            output.operator_commands,
            vec![OperatorCommand::CancelCall {
                call_id: "call-a".into()
            }]
        );
    }

    #[test]
    fn operator_incoming_builds_uac_invite_and_maps_answer() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven)
        .with_asterisk_target("sip:6108@192.0.2.20:8060");
        let output = bridge
            .start_operator_incoming("ims-call-a", "sip:+8613800@ims.example", sdp())
            .unwrap();
        let invite = &output.asterisk_frames[0];
        assert!(
            String::from_utf8_lossy(invite).starts_with("INVITE sip:6108@192.0.2.20:8060 SIP/2.0")
        );
        let call_id = dialog::call_id(invite).unwrap();
        let response = format!(
            "SIP/2.0 200 OK\r\nVia: {}\r\nFrom: {}\r\nTo: <sip:6108@192.0.2.20:8060>;tag=pbx-answer\r\nCall-ID: {}\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            sip_frame::header_value(invite, "Via").unwrap(),
            sip_frame::header_value(invite, "From").unwrap(),
            call_id,
            sdp().len(),
            String::from_utf8_lossy(sdp()),
        );
        let output = bridge.handle_asterisk(response.as_bytes()).unwrap();
        assert!(output.asterisk_frames[0].starts_with(b"ACK "));
        assert_eq!(
            output.operator_commands,
            vec![OperatorCommand::AcceptCall {
                call_id: "ims-call-a".into(),
                body: sdp().to_vec(),
            }]
        );
    }
}
