//! Asterisk ↔ operator IMS control-plane bridge.
//!
//! This is intentionally an event-driven B2BUA seam. An Asterisk INVITE is
//! parsed and acknowledged immediately, then translated into an
//! [`OperatorCommand`]. The per-line VoLTE live loop feeds the resulting
//! [`OperatorEvent`] back into the bridge, which emits the corresponding SIP
//! response or in-dialog request. No task waits synchronously for the modem.
//! Availability remains closed until a real live command consumer subscribes,
//! so an offline IMS leg still receives an honest 480 after 100 Trying.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
};

use crate::{
    connectivity::core::{
        ims_video::{parse_video_sdp, VideoMediaDescription},
        sip_frame,
        sip_message::SipHeader,
        supplementary::{
            normalize_refer_target, DialogTransfer, DialogTransferState, ReferNotification,
            ReferSubscriptionState,
        },
        voice::{parse_audio_sdp, SdpAudioDescription},
    },
    services::trunk::{
        dialog::{self, InviteTransactionState, SipDialog},
        digest,
        sip::{self, DialogRequest},
    },
};

const MAX_INVITE_DIGEST_ROUNDS: u32 = 2;

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
    pub dtmf: DtmfCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoOffer {
    pub description: VideoMediaDescription,
    pub endpoint: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtmfCapabilities {
    /// RFC 4733/RFC 2833 dynamic payload negotiated in the audio m-line.
    pub rtp_event: Option<RtpTelephoneEvent>,
    /// The trunk endpoint always accepts RFC 2976-style SIP INFO as fallback.
    pub sip_info: bool,
    pub preferred: DtmfSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpTelephoneEvent {
    pub payload_type: u8,
    pub clock_rate: u32,
    pub events: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtmfSource {
    SipInfo,
    RtpEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtmfSignal {
    pub digit: char,
    pub duration_ms: u16,
    pub source: DtmfSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorCommand {
    StartCall {
        call_id: String,
        caller: String,
        callee: String,
        trunk_local_ip: IpAddr,
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
        trunk_local_ip: IpAddr,
        offer: MediaOffer,
    },
    AcceptRenegotiation {
        call_id: String,
        body: Vec<u8>,
    },
    RejectRenegotiation {
        call_id: String,
        status: u16,
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
    SendDtmf {
        call_id: String,
        signal: DtmfSignal,
    },
    TransferCall {
        call_id: String,
        refer_to: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorEvent {
    Incoming {
        call_id: String,
        caller: String,
        body: Vec<u8>,
    },
    Provisional {
        call_id: String,
        status: u16,
        body: Option<Vec<u8>>,
    },
    Answered {
        call_id: String,
        body: Vec<u8>,
    },
    Renegotiate {
        call_id: String,
        body: Vec<u8>,
    },
    Dtmf {
        call_id: String,
        signal: DtmfSignal,
    },
    TransferResponse {
        call_id: String,
        status: u16,
    },
    TransferNotify {
        call_id: String,
        notification: ReferNotification,
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
    Cancelled {
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
    Forbidden(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedRequest(reason) => write!(f, "malformed trunk request: {reason}"),
            Self::InvalidState(reason) => write!(f, "invalid trunk bridge state: {reason}"),
            Self::UnsupportedMedia(reason) => write!(f, "unsupported trunk media: {reason}"),
            Self::Forbidden(reason) => write!(f, "forbidden trunk request: {reason}"),
        }
    }
}

impl std::error::Error for BridgeError {}

#[derive(Debug, Clone)]
struct BridgedCall {
    dialog: SipDialog,
    operator_call_id: String,
    invite_digest_rounds: u32,
    pending_invite: Option<Vec<u8>>,
    operator_reinvite: Option<Vec<u8>>,
    transfer: Option<BridgedTransfer>,
    hangup_after_ack: bool,
}

#[derive(Debug, Clone)]
struct BridgedTransfer {
    refer_request: Vec<u8>,
    state: DialogTransfer,
    asterisk_event_id: u32,
}

#[derive(Debug, Clone)]
pub struct TrunkBridge {
    local_addr: SocketAddr,
    local_aor: String,
    asterisk_target: Option<String>,
    outgoing_binding: Option<String>,
    digest_username: Option<String>,
    digest_secret: Option<String>,
    operator: OperatorAvailability,
    calls: HashMap<String, BridgedCall>,
}

impl TrunkBridge {
    pub fn new(local_addr: SocketAddr, local_aor: impl Into<String>) -> Self {
        Self {
            local_addr,
            local_aor: local_aor.into(),
            asterisk_target: None,
            outgoing_binding: None,
            digest_username: None,
            digest_secret: None,
            operator: OperatorAvailability::Unavailable,
            calls: HashMap::new(),
        }
    }

    pub fn with_operator(mut self, operator: OperatorAvailability) -> Self {
        self.operator = operator;
        self
    }

    pub fn set_operator(&mut self, operator: OperatorAvailability) {
        self.operator = operator;
    }

    pub fn with_asterisk_target(mut self, target: impl Into<String>) -> Self {
        self.asterisk_target = Some(target.into());
        self
    }

    pub fn with_outgoing_binding(mut self, binding: impl Into<String>) -> Self {
        let binding = binding.into();
        self.outgoing_binding = (!binding.trim().is_empty()).then(|| binding.trim().to_string());
        self
    }

    pub fn with_digest_credentials(
        mut self,
        username: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        let username = username.into();
        let secret = secret.into();
        self.digest_username = (!username.trim().is_empty()).then(|| username.trim().to_string());
        self.digest_secret = (!secret.is_empty()).then_some(secret);
        self
    }

    /// Start a mobile-terminated call toward the configured Asterisk target.
    /// The returned frame is a normal UAC INVITE; subsequent Asterisk
    /// responses are fed through [`handle_asterisk`].
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
                invite_digest_rounds: 0,
                pending_invite: None,
                operator_reinvite: None,
                transfer: None,
                hangup_after_ack: false,
            },
        );
        Ok(BridgeOutput {
            asterisk_frames: vec![outbound_frame],
            ..BridgeOutput::default()
        })
    }

    pub fn active_call_count(&self) -> usize {
        self.calls.len()
    }

    pub fn confirmed_call_count(&self) -> usize {
        self.calls
            .values()
            .filter(|call| call.dialog.state == InviteTransactionState::Confirmed)
            .count()
    }

    pub fn has_call(&self, call_id: &str) -> bool {
        self.calls.contains_key(call_id)
    }

    pub fn handle_asterisk(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        if !sip::is_request(frame) {
            if let Some(output) = self.handle_invite_digest_challenge(frame) {
                return Ok(output);
            }
            return Ok(self.handle_asterisk_response(frame));
        }
        let method = first_token(frame).unwrap_or_default();
        match method.as_str() {
            "INVITE" => self.handle_invite(frame),
            "ACK" => self.handle_ack(frame),
            "CANCEL" => self.handle_cancel(frame),
            "BYE" => self.handle_bye(frame),
            "INFO" => self.handle_info(frame),
            "REFER" => self.handle_refer(frame),
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
        if let OperatorEvent::Incoming {
            call_id,
            caller,
            body,
        } = event
        {
            return self.start_operator_incoming(call_id, &caller, &body);
        }
        let call_id = match &event {
            OperatorEvent::Incoming { .. } => unreachable!("handled above"),
            OperatorEvent::Provisional { call_id, .. }
            | OperatorEvent::Answered { call_id, .. }
            | OperatorEvent::Renegotiate { call_id, .. }
            | OperatorEvent::Dtmf { call_id, .. }
            | OperatorEvent::TransferResponse { call_id, .. }
            | OperatorEvent::TransferNotify { call_id, .. }
            | OperatorEvent::Rejected { call_id, .. }
            | OperatorEvent::Unavailable { call_id }
            | OperatorEvent::Ended { call_id }
            | OperatorEvent::Cancelled { call_id } => call_id,
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
        let response_request = call
            .pending_invite
            .clone()
            .unwrap_or_else(|| call.dialog.initial_invite.clone());
        match event {
            OperatorEvent::Incoming { .. } => unreachable!("handled above"),
            OperatorEvent::Provisional { status, body, .. } => {
                if call.pending_invite.is_none() {
                    call.dialog
                        .on_provisional(status)
                        .map_err(BridgeError::InvalidState)?;
                }
                let body = body.unwrap_or_default();
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &response_request,
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
                if call.pending_invite.is_none() {
                    call.dialog
                        .on_final(200)
                        .map_err(BridgeError::InvalidState)?;
                }
                let contact = SipHeader::new("Contact", format!("<{}>", self.local_aor));
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &response_request,
                        200,
                        "OK",
                        Some(&call.dialog.local_tag),
                        &[contact],
                        &body,
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
                call.pending_invite = None;
            }
            OperatorEvent::Renegotiate { body, .. } => {
                if call.dialog.state != InviteTransactionState::Confirmed
                    || call.pending_invite.is_some()
                    || call.operator_reinvite.is_some()
                {
                    output
                        .operator_commands
                        .push(OperatorCommand::RejectRenegotiation {
                            call_id: call.operator_call_id.clone(),
                            status: 491,
                        });
                } else {
                    let cseq = call
                        .dialog
                        .begin_local_request()
                        .map_err(BridgeError::InvalidState)?;
                    let reinvite = sip::build_dialog_request(&DialogRequest {
                        method: "INVITE",
                        request_uri: &call.dialog.remote_uri,
                        local_addr: self.local_addr,
                        from_uri: &call.dialog.local_uri,
                        from_tag: &call.dialog.local_tag,
                        to_uri: &call.dialog.remote_uri,
                        to_tag: call.dialog.remote_tag.as_deref(),
                        call_id: &call.dialog.call_id,
                        cseq,
                        contact_uri: Some(&self.local_aor),
                        body: &body,
                    })
                    .map_err(BridgeError::MalformedRequest)?;
                    call.operator_reinvite = Some(reinvite.clone());
                    output.asterisk_frames.push(reinvite);
                }
            }
            OperatorEvent::Dtmf { signal, .. } => {
                if call.dialog.state != InviteTransactionState::Confirmed {
                    return Err(BridgeError::InvalidState(
                        "operator_dtmf_before_confirmed".to_string(),
                    ));
                }
                let cseq = call
                    .dialog
                    .begin_local_request()
                    .map_err(BridgeError::InvalidState)?;
                let body = format!(
                    "Signal={}\r\nDuration={}\r\n",
                    signal.digit.to_ascii_uppercase(),
                    signal.duration_ms
                );
                output.asterisk_frames.push(
                    sip::build_dialog_request_with_content_type(
                        &DialogRequest {
                            method: "INFO",
                            request_uri: &call.dialog.remote_uri,
                            local_addr: self.local_addr,
                            from_uri: &call.dialog.local_uri,
                            from_tag: &call.dialog.local_tag,
                            to_uri: &call.dialog.remote_uri,
                            to_tag: call.dialog.remote_tag.as_deref(),
                            call_id: &call.dialog.call_id,
                            cseq,
                            contact_uri: None,
                            body: body.as_bytes(),
                        },
                        Some("application/dtmf-relay"),
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::TransferResponse { status, .. } => {
                let transfer = call.transfer.as_mut().ok_or_else(|| {
                    BridgeError::InvalidState("operator_transfer_not_pending".to_string())
                })?;
                transfer
                    .state
                    .on_refer_response(status)
                    .map_err(|error| BridgeError::InvalidState(error.to_string()))?;
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &transfer.refer_request,
                        status,
                        reason(status),
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::TransferNotify { notification, .. } => {
                let transfer = call.transfer.as_mut().ok_or_else(|| {
                    BridgeError::InvalidState("operator_transfer_not_pending".to_string())
                })?;
                if transfer.state.state() == DialogTransferState::Pending {
                    transfer
                        .state
                        .on_refer_response(202)
                        .map_err(|error| BridgeError::InvalidState(error.to_string()))?;
                    output.asterisk_frames.push(
                        sip::build_response_with_body(
                            &transfer.refer_request,
                            202,
                            "Accepted",
                            Some(&call.dialog.local_tag),
                            &[],
                            &[],
                        )
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                }
                transfer
                    .state
                    .on_notify(&notification)
                    .map_err(|error| BridgeError::InvalidState(error.to_string()))?;
                let cseq = call
                    .dialog
                    .begin_local_request()
                    .map_err(BridgeError::InvalidState)?;
                let subscription_state = match notification.subscription_state {
                    ReferSubscriptionState::Pending => "pending",
                    ReferSubscriptionState::Active => "active",
                    ReferSubscriptionState::Terminated => "terminated;reason=noresource",
                };
                let body = format!(
                    "SIP/2.0 {} {}\r\n",
                    notification.sip_status,
                    reason(notification.sip_status)
                );
                let event = format!("refer;id={}", transfer.asterisk_event_id);
                let headers = [
                    SipHeader::new("Event", event),
                    SipHeader::new("Subscription-State", subscription_state),
                ];
                output.asterisk_frames.push(
                    sip::build_dialog_request_with_headers_and_content_type(
                        &DialogRequest {
                            method: "NOTIFY",
                            request_uri: &call.dialog.remote_target,
                            local_addr: self.local_addr,
                            from_uri: &call.dialog.local_uri,
                            from_tag: &call.dialog.local_tag,
                            to_uri: &call.dialog.remote_uri,
                            to_tag: call.dialog.remote_tag.as_deref(),
                            call_id: &call.dialog.call_id,
                            cseq,
                            contact_uri: Some(&self.local_aor),
                            body: body.as_bytes(),
                        },
                        &headers,
                        Some("message/sipfrag;version=2.0"),
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
            }
            OperatorEvent::Rejected { status, .. } => {
                if call.pending_invite.is_some() {
                    output.asterisk_frames.push(
                        sip::build_response_with_body(
                            &response_request,
                            status,
                            reason(status),
                            Some(&call.dialog.local_tag),
                            &[],
                            &[],
                        )
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                } else if call.dialog.state == InviteTransactionState::Confirmed {
                    let cseq = call
                        .dialog
                        .begin_local_request()
                        .map_err(BridgeError::InvalidState)?;
                    output.asterisk_frames.push(
                        sip::build_dialog_request(&DialogRequest {
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
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                    call.dialog.state = InviteTransactionState::Terminated;
                } else if call.dialog.state == InviteTransactionState::AcceptedAwaitingAck {
                    call.hangup_after_ack = true;
                } else {
                    call.dialog
                        .on_final(status)
                        .map_err(BridgeError::InvalidState)?;
                    output.asterisk_frames.push(
                        sip::build_response_with_body(
                            &response_request,
                            status,
                            reason(status),
                            Some(&call.dialog.local_tag),
                            &[],
                            &[],
                        )
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                }
                call.pending_invite = None;
            }
            OperatorEvent::Unavailable { .. } => {
                if call.pending_invite.is_none() {
                    call.dialog
                        .on_final(480)
                        .map_err(BridgeError::InvalidState)?;
                }
                output.asterisk_frames.push(
                    sip::build_response_with_body(
                        &response_request,
                        480,
                        "Temporarily Unavailable",
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?,
                );
                call.pending_invite = None;
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
                } else if call.dialog.state == InviteTransactionState::AcceptedAwaitingAck {
                    call.hangup_after_ack = true;
                } else if call.dialog.direction == dialog::DialogDirection::OperatorOriginated
                    && call.dialog.state == InviteTransactionState::Proceeding
                {
                    output.asterisk_frames.push(
                        sip::build_cancel(&call.dialog.initial_invite)
                            .map_err(BridgeError::MalformedRequest)?,
                    );
                    call.dialog.state = InviteTransactionState::Failed;
                } else if call.dialog.direction == dialog::DialogDirection::AsteriskOriginated
                    && call.dialog.state == InviteTransactionState::Proceeding
                {
                    call.dialog
                        .on_final(487)
                        .map_err(BridgeError::InvalidState)?;
                    output.asterisk_frames.push(
                        sip::build_response_with_body(
                            &response_request,
                            487,
                            "Request Terminated",
                            Some(&call.dialog.local_tag),
                            &[],
                            &[],
                        )
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                }
            }
            OperatorEvent::Cancelled { .. } => {
                if call.dialog.direction == dialog::DialogDirection::OperatorOriginated
                    && call.dialog.state == InviteTransactionState::Proceeding
                {
                    output.asterisk_frames.push(
                        sip::build_cancel(&call.dialog.initial_invite)
                            .map_err(BridgeError::MalformedRequest)?,
                    );
                    call.dialog.state = InviteTransactionState::Failed;
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
            if call.operator_reinvite.is_some() || call.pending_invite.is_some() {
                return Ok(BridgeOutput {
                    asterisk_frames: vec![sip::build_response_with_body(
                        frame,
                        491,
                        "Request Pending",
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?],
                    ..BridgeOutput::default()
                });
            }
            let offer = parse_offer(frame)?;
            let cseq =
                dialog::cseq_number(frame, "INVITE").map_err(BridgeError::MalformedRequest)?;
            call.dialog.next_local_cseq = cseq.saturating_add(1);
            call.pending_invite = Some(frame.to_vec());
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
                operator_commands: vec![OperatorCommand::Renegotiate {
                    call_id,
                    trunk_local_ip: self.local_addr.ip(),
                    offer,
                }],
            });
        }

        let offer = parse_offer(frame)?;
        let dialog =
            SipDialog::from_asterisk_invite(frame).map_err(BridgeError::MalformedRequest)?;
        let caller = sip_frame::header_uri(frame, "From").unwrap_or_else(|| "sip:unknown".into());
        if let Some(binding) = self.outgoing_binding.as_deref() {
            let caller_user = sip_user(&caller).unwrap_or_default();
            if caller_user != binding {
                return Err(BridgeError::Forbidden(
                    "trunk_outgoing_binding_mismatch".into(),
                ));
            }
        }
        let callee = request_uri(frame)
            .or_else(|| sip_frame::header_uri(frame, "To"))
            .unwrap_or_else(|| self.local_aor.clone());
        let command = OperatorCommand::StartCall {
            call_id: call_id.clone(),
            caller,
            callee,
            trunk_local_ip: self.local_addr.ip(),
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
                invite_digest_rounds: 0,
                pending_invite: None,
                operator_reinvite: None,
                transfer: None,
                hangup_after_ack: false,
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
        let mut output = BridgeOutput::default();
        let mut remove = false;
        if let Some(call) = self.calls.get_mut(&call_id) {
            if call.dialog.state == InviteTransactionState::AcceptedAwaitingAck {
                call.dialog.on_ack().map_err(BridgeError::InvalidState)?;
                if call.hangup_after_ack {
                    let cseq = call
                        .dialog
                        .begin_local_request()
                        .map_err(BridgeError::InvalidState)?;
                    output.asterisk_frames.push(
                        sip::build_dialog_request(&DialogRequest {
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
                        .map_err(BridgeError::MalformedRequest)?,
                    );
                    call.dialog.state = InviteTransactionState::Terminated;
                    remove = true;
                }
            }
        }
        if remove {
            self.calls.remove(&call_id);
        }
        Ok(output)
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

    fn handle_info(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_info_call-id_missing".into()))?;
        let Some(call) = self.calls.get(&call_id) else {
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
        if call.dialog.state != InviteTransactionState::Confirmed {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response_with_body(
                    frame,
                    481,
                    "Call/Transaction Does Not Exist",
                    Some(&call.dialog.local_tag),
                    &[],
                    &[],
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        }
        let signal = match parse_dtmf_info(frame) {
            Ok(signal) => signal,
            Err(DtmfInfoError::UnsupportedContentType) => {
                return Ok(BridgeOutput {
                    asterisk_frames: vec![sip::build_response_with_body(
                        frame,
                        415,
                        "Unsupported Media Type",
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?],
                    ..BridgeOutput::default()
                });
            }
            Err(DtmfInfoError::Malformed) => {
                return Ok(BridgeOutput {
                    asterisk_frames: vec![sip::build_response_with_body(
                        frame,
                        400,
                        "Bad Request",
                        Some(&call.dialog.local_tag),
                        &[],
                        &[],
                    )
                    .map_err(BridgeError::MalformedRequest)?],
                    ..BridgeOutput::default()
                });
            }
        };
        Ok(BridgeOutput {
            asterisk_frames: vec![sip::build_response_with_body(
                frame,
                200,
                "OK",
                Some(&call.dialog.local_tag),
                &[],
                &[],
            )
            .map_err(BridgeError::MalformedRequest)?],
            operator_commands: vec![OperatorCommand::SendDtmf {
                call_id: call.operator_call_id.clone(),
                signal,
            }],
        })
    }

    fn handle_refer(&mut self, frame: &[u8]) -> Result<BridgeOutput, BridgeError> {
        let call_id = dialog::call_id(frame)
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_refer_call-id_missing".into()))?;
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
        if call.dialog.state != InviteTransactionState::Confirmed {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response_with_body(
                    frame,
                    481,
                    "Call/Transaction Does Not Exist",
                    Some(&call.dialog.local_tag),
                    &[],
                    &[],
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        }
        if call
            .transfer
            .as_ref()
            .is_some_and(|transfer| !transfer.state.state().is_terminal())
        {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response_with_body(
                    frame,
                    491,
                    "Request Pending",
                    Some(&call.dialog.local_tag),
                    &[],
                    &[],
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        }
        let refer_to = sip_frame::header_value(frame, "Refer-To")
            .ok_or_else(|| BridgeError::MalformedRequest("trunk_refer_to_missing".into()))?;
        let asterisk_event_id =
            dialog::cseq_number(frame, "REFER").map_err(BridgeError::MalformedRequest)?;
        let refer_to = normalize_refer_target(&refer_to)
            .map_err(|error| BridgeError::MalformedRequest(error.to_string()))?;
        if refer_to.split_once('?').is_some_and(|(_, query)| {
            query.split('&').any(|parameter| {
                parameter
                    .split_once('=')
                    .map(|(name, _)| name)
                    .unwrap_or(parameter)
                    .eq_ignore_ascii_case("replaces")
            })
        }) {
            return Ok(BridgeOutput {
                asterisk_frames: vec![sip::build_response_with_body(
                    frame,
                    501,
                    "Not Implemented",
                    Some(&call.dialog.local_tag),
                    &[],
                    &[],
                )
                .map_err(BridgeError::MalformedRequest)?],
                ..BridgeOutput::default()
            });
        }
        call.transfer = Some(BridgedTransfer {
            refer_request: frame.to_vec(),
            state: DialogTransfer::default(),
            asterisk_event_id,
        });
        Ok(BridgeOutput {
            operator_commands: vec![OperatorCommand::TransferCall {
                call_id: call.operator_call_id.clone(),
                refer_to,
            }],
            ..BridgeOutput::default()
        })
    }

    fn handle_invite_digest_challenge(&mut self, frame: &[u8]) -> Option<BridgeOutput> {
        let status = sip::status(frame).ok()?;
        if status != 401 && status != 407 {
            return None;
        }
        let method = sip_frame::header_value(frame, "CSeq")?
            .split_whitespace()
            .nth(1)?
            .to_string();
        if !method.eq_ignore_ascii_case("INVITE") {
            return None;
        }
        let call_id = dialog::call_id(frame)?;
        let username = self.digest_username.as_deref()?;
        let secret = self.digest_secret.as_deref()?;
        let call = self.calls.get_mut(&call_id)?;
        if call.dialog.direction != dialog::DialogDirection::OperatorOriginated
            || call.dialog.state != InviteTransactionState::Proceeding
            || call.operator_reinvite.is_some()
            || call.invite_digest_rounds >= MAX_INVITE_DIGEST_ROUNDS
            || dialog::cseq_number(frame, "INVITE").ok()? != call.dialog.invite_cseq
        {
            return None;
        }
        let proxy = status == 407;
        let challenge_header = if proxy {
            "Proxy-Authenticate"
        } else {
            "WWW-Authenticate"
        };
        let challenge =
            digest::parse_challenge(&sip_frame::header_value(frame, challenge_header)?, proxy)
                .ok()?;
        let digest_uri = dialog::request_uri(&call.dialog.initial_invite).ok()?;
        let next_round = call.invite_digest_rounds.saturating_add(1);
        let authorization = digest::build_authorization(
            &challenge,
            username,
            secret,
            "INVITE",
            &digest_uri,
            &sip::token(12),
            next_round,
        )
        .ok()?;
        let ack = sip::build_ack_for_final(&call.dialog.initial_invite, frame).ok()?;
        let retry =
            sip::build_authenticated_invite_retry(&call.dialog.initial_invite, &authorization)
                .ok()?;
        let retry_cseq = dialog::cseq_number(&retry, "INVITE").ok()?;
        call.dialog.invite_cseq = retry_cseq;
        call.dialog.next_local_cseq = retry_cseq.saturating_add(1);
        call.dialog.initial_invite = retry.clone();
        call.invite_digest_rounds = next_round;
        Some(BridgeOutput {
            asterisk_frames: vec![ack, retry],
            ..BridgeOutput::default()
        })
    }

    fn handle_asterisk_response(&mut self, frame: &[u8]) -> BridgeOutput {
        let Some(call_id) = dialog::call_id(frame) else {
            return BridgeOutput::default();
        };
        let status = sip::status(frame).unwrap_or(0);
        let method = sip_frame::header_value(frame, "CSeq")
            .and_then(|value| value.split_whitespace().nth(1).map(str::to_string));
        if !method.is_some_and(|method| method.eq_ignore_ascii_case("INVITE")) {
            return BridgeOutput::default();
        }
        let mut remove_call = false;
        let output = {
            let Some(call) = self.calls.get_mut(&call_id) else {
                return BridgeOutput::default();
            };
            if let Some(reinvite) = call.operator_reinvite.clone() {
                if (100..200).contains(&status) {
                    return BridgeOutput::default();
                }
                call.operator_reinvite = None;
                let ack = sip::build_ack_for_final(&reinvite, frame).ok();
                if (200..300).contains(&status) {
                    BridgeOutput {
                        asterisk_frames: ack.into_iter().collect(),
                        operator_commands: vec![OperatorCommand::AcceptRenegotiation {
                            call_id: call.operator_call_id.clone(),
                            body: sip_frame::body(frame).to_vec(),
                        }],
                    }
                } else {
                    BridgeOutput {
                        asterisk_frames: ack.into_iter().collect(),
                        operator_commands: vec![OperatorCommand::RejectRenegotiation {
                            call_id: call.operator_call_id.clone(),
                            status,
                        }],
                    }
                }
            } else if call.dialog.direction != dialog::DialogDirection::OperatorOriginated {
                return BridgeOutput::default();
            } else if (100..200).contains(&status) {
                let _ = call.dialog.on_provisional(status);
                BridgeOutput {
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
                }
            } else if (200..300).contains(&status) {
                let _ = call.dialog.on_final(status);
                call.dialog.learn_remote_tag(frame);
                call.dialog.learn_remote_target(frame);
                let ack = sip::build_ack_for_final(&call.dialog.initial_invite, frame).ok();
                call.dialog.state = InviteTransactionState::Confirmed;
                BridgeOutput {
                    asterisk_frames: ack.into_iter().collect(),
                    operator_commands: vec![OperatorCommand::AcceptCall {
                        call_id: call.operator_call_id.clone(),
                        body: sip_frame::body(frame).to_vec(),
                    }],
                }
            } else {
                let ack = sip::build_ack_for_final(&call.dialog.initial_invite, frame).ok();
                call.dialog.state = InviteTransactionState::Failed;
                remove_call = true;
                BridgeOutput {
                    asterisk_frames: ack.into_iter().collect(),
                    operator_commands: vec![OperatorCommand::RejectCall {
                        call_id: call.operator_call_id.clone(),
                        status,
                    }],
                }
            }
        };
        if remove_call {
            self.calls.remove(&call_id);
        }
        output
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
            let video_address =
                media_connection_address(body, "video").unwrap_or(&audio.connection_addr);
            let endpoint = media_endpoint(video_address, description.media_port)
                .map_err(|error| BridgeError::UnsupportedMedia(error.to_string()))?;
            Ok(VideoOffer {
                description,
                endpoint,
            })
        })
        .transpose()?;
    let rtp_event = parse_rtp_telephone_event(body);
    let preferred = if rtp_event.is_some() {
        DtmfSource::RtpEvent
    } else {
        DtmfSource::SipInfo
    };
    Ok(MediaOffer {
        audio,
        audio_endpoint,
        video,
        dtmf: DtmfCapabilities {
            rtp_event,
            sip_info: true,
            preferred,
        },
    })
}

fn media_connection_address<'a>(body: &'a [u8], media_kind: &str) -> Option<&'a str> {
    let text = std::str::from_utf8(body).ok()?;
    let mut current_media = None;
    let mut session_connection = None;
    let mut media_connection = None;
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r').trim();
        if let Some(value) = line.strip_prefix("m=") {
            current_media = value.split_whitespace().next();
        } else if let Some(value) = line.strip_prefix("c=") {
            let address = value.split_whitespace().nth(2)?;
            match current_media {
                None => session_connection = Some(address),
                Some(kind) if kind.eq_ignore_ascii_case(media_kind) => {
                    media_connection = Some(address)
                }
                _ => {}
            }
        }
    }
    media_connection.or(session_connection)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DtmfInfoError {
    UnsupportedContentType,
    Malformed,
}

fn parse_dtmf_info(frame: &[u8]) -> Result<DtmfSignal, DtmfInfoError> {
    let content_type = sip_frame::header_value(frame, "Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let body = std::str::from_utf8(sip_frame::body(frame)).map_err(|_| DtmfInfoError::Malformed)?;
    let (digit, duration_ms) = match content_type.as_str() {
        "application/dtmf-relay" => {
            let mut digit = None;
            let mut duration = None;
            for line in body.lines() {
                let Some((name, value)) = line.trim().split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("signal") {
                    digit = value.trim().chars().next();
                } else if name.trim().eq_ignore_ascii_case("duration") {
                    duration = value.trim().parse::<u16>().ok();
                }
            }
            (
                digit.ok_or(DtmfInfoError::Malformed)?,
                duration.unwrap_or(160),
            )
        }
        "application/dtmf" => {
            let digit = body.trim().chars().next().ok_or(DtmfInfoError::Malformed)?;
            (digit, 160)
        }
        _ => return Err(DtmfInfoError::UnsupportedContentType),
    };
    if !is_dtmf_digit(digit) || !(40..=5000).contains(&duration_ms) {
        return Err(DtmfInfoError::Malformed);
    }
    Ok(DtmfSignal {
        digit,
        duration_ms,
        source: DtmfSource::SipInfo,
    })
}

pub(crate) fn parse_operator_dtmf_info(frame: &[u8]) -> Option<DtmfSignal> {
    parse_dtmf_info(frame).ok()
}

pub(crate) fn parse_rtp_telephone_event(body: &[u8]) -> Option<RtpTelephoneEvent> {
    let text = std::str::from_utf8(body).ok()?;
    let mut in_audio = false;
    let mut event = None;
    let mut fmtps = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r').trim();
        if let Some(media) = line.strip_prefix("m=") {
            in_audio = media.split_whitespace().next() == Some("audio");
            continue;
        }
        if !in_audio {
            continue;
        }
        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let mut fields = value.split_whitespace();
            let Some(payload_type) = fields.next().and_then(|value| value.parse::<u8>().ok())
            else {
                continue;
            };
            if payload_type > 0x7f {
                continue;
            }
            let Some(encoding) = fields.next() else {
                continue;
            };
            let mut encoding_fields = encoding.split('/');
            if encoding_fields
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("telephone-event"))
            {
                let Some(clock_rate) = encoding_fields
                    .next()
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    continue;
                };
                if clock_rate == 0 {
                    continue;
                }
                event = Some((payload_type, clock_rate));
            }
        } else if let Some(value) = line.strip_prefix("a=fmtp:") {
            if let Some((payload_type, params)) = value.split_once(char::is_whitespace) {
                if let Ok(payload_type) = payload_type.parse::<u8>() {
                    fmtps.push((payload_type, params.trim().to_string()));
                }
            }
        }
    }
    let (payload_type, clock_rate) = event?;
    let events = fmtps
        .into_iter()
        .find(|(fmtp_payload, _)| *fmtp_payload == payload_type)
        .map(|(_, params)| params);
    Some(RtpTelephoneEvent {
        payload_type,
        clock_rate,
        events,
    })
}

fn is_dtmf_digit(digit: char) -> bool {
    matches!(digit.to_ascii_uppercase(), '0'..='9' | '*' | '#' | 'A'..='D')
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

fn sip_user(uri: &str) -> Option<&str> {
    uri.strip_prefix("sip:")?
        .split(['@', ';'])
        .next()
        .filter(|user| !user.is_empty())
}

fn first_token(frame: &[u8]) -> Option<String> {
    frame
        .split(|byte| *byte == b' ')
        .next()
        .and_then(|token| std::str::from_utf8(token).ok())
        .map(str::to_string)
}

fn request_uri(frame: &[u8]) -> Option<String> {
    let first_line = std::str::from_utf8(frame).ok()?.lines().next()?;
    let mut fields = first_line.split_whitespace();
    fields.next()?;
    let uri = fields.next()?;
    uri.starts_with("sip:").then(|| uri.to_string())
}

fn reason(status: u16) -> &'static str {
    match status {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        202 => "Accepted",
        408 => "Request Timeout",
        480 => "Temporarily Unavailable",
        481 => "Call/Transaction Does Not Exist",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        491 => "Request Pending",
        503 => "Service Unavailable",
        _ => "Failure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    fn sdp() -> &'static [u8] {
        b"v=0\r\no=- 1 1 IN IP4 192.0.2.10\r\ns=call\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n"
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
        let OperatorCommand::StartCall { offer, .. } = &output.operator_commands[0] else {
            panic!("expected start call");
        };
        assert_eq!(
            offer.dtmf.rtp_event,
            Some(RtpTelephoneEvent {
                payload_type: 101,
                clock_rate: 8000,
                events: Some("0-16".into()),
            })
        );
        assert!(offer.dtmf.sip_info);
        assert_eq!(offer.dtmf.preferred, DtmfSource::RtpEvent);
        assert!(parse_rtp_telephone_event(
            b"m=audio 40000 RTP/AVP 200\r\na=rtpmap:200 telephone-event/8000\r\n"
        )
        .is_none());
        assert!(parse_rtp_telephone_event(
            b"m=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\nm=video 50000 RTP/AVP 101\r\na=rtpmap:101 telephone-event/8000\r\n"
        )
        .is_none());
        assert!(String::from_utf8_lossy(&output.asterisk_frames[0]).starts_with("SIP/2.0 100"));
        assert!(String::from_utf8_lossy(&output.asterisk_frames[1]).starts_with("SIP/2.0 480"));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn media_offer_honors_separate_audio_and_video_connection_addresses() {
        let body = b"v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=call\r\nc=IN IP4 192.0.2.2\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\nc=IN IP4 192.0.2.10\r\na=rtpmap:0 PCMU/8000\r\nm=video 40002 RTP/AVP 96\r\nc=IN IP4 192.0.2.20\r\na=rtpmap:96 H264/90000\r\n";
        let offer = parse_media_offer(body).unwrap();
        assert_eq!(
            offer.audio_endpoint,
            "192.0.2.10:40000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            offer.video.unwrap().endpoint,
            "192.0.2.20:40002".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn outgoing_binding_allows_only_matching_asterisk_from_user() {
        let mut allowed = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven)
        .with_outgoing_binding("6108");
        let output = allowed.handle_asterisk(&invite()).unwrap();
        assert!(matches!(
            output.operator_commands.as_slice(),
            [OperatorCommand::StartCall { .. }]
        ));

        let mut rejected = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven)
        .with_outgoing_binding("6109");
        assert!(matches!(
            rejected.handle_asterisk(&invite()),
            Err(BridgeError::Forbidden(reason)) if reason == "trunk_outgoing_binding_mismatch"
        ));
        assert_eq!(rejected.active_call_count(), 0);
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
    fn operator_end_before_ip_answer_terminates_asterisk_invite() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        let output = bridge
            .handle_operator_event(OperatorEvent::Ended {
                call_id: "call-a".into(),
            })
            .unwrap();
        assert_eq!(output.asterisk_frames.len(), 1);
        assert!(output.asterisk_frames[0].starts_with(b"SIP/2.0 487 Request Terminated"));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn operator_rejection_after_first_rtp_answers_then_hangs_up_after_ack() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        let answered = bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        assert!(answered.asterisk_frames[0].starts_with(b"SIP/2.0 200 OK"));

        let rejected = bridge
            .handle_operator_event(OperatorEvent::Rejected {
                call_id: "call-a".into(),
                status: 486,
            })
            .unwrap();
        assert!(rejected.asterisk_frames.is_empty());
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        let output = bridge.handle_asterisk(ack).unwrap();
        assert_eq!(output.asterisk_frames.len(), 1);
        assert!(output.asterisk_frames[0].starts_with(b"BYE "));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn confirmed_reinvite_response_targets_pending_transaction() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let reinvite = String::from_utf8(invite())
            .unwrap()
            .replace("CSeq: 1 INVITE", "CSeq: 2 INVITE");
        let output = bridge.handle_asterisk(reinvite.as_bytes()).unwrap();
        assert!(matches!(
            output.operator_commands.as_slice(),
            [OperatorCommand::Renegotiate { call_id, .. }] if call_id == "call-a"
        ));
        let answer = bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let response = String::from_utf8_lossy(&answer.asterisk_frames[0]);
        assert!(response.starts_with("SIP/2.0 200 OK"));
        assert!(response.contains("CSeq: 2 INVITE\r\n"));
        assert_eq!(bridge.active_call_count(), 1);
    }

    #[test]
    fn operator_reinvite_round_trips_through_asterisk_dialog() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();

        let output = bridge
            .handle_operator_event(OperatorEvent::Renegotiate {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let request = &output.asterisk_frames[0];
        assert!(request.starts_with(b"INVITE "));
        let response = format!(
            "SIP/2.0 200 OK\r\nVia: {}\r\nFrom: {}\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {}\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            sip_frame::header_value(request, "Via").unwrap(),
            sip_frame::header_value(request, "From").unwrap(),
            sip_frame::header_value(request, "To").unwrap(),
            sip_frame::header_value(request, "Call-ID").unwrap(),
            sip_frame::header_value(request, "CSeq").unwrap(),
            sdp().len(),
            String::from_utf8_lossy(sdp()),
        );
        let answered = bridge.handle_asterisk(response.as_bytes()).unwrap();
        assert!(answered.asterisk_frames[0].starts_with(b"ACK "));
        assert!(matches!(
            answered.operator_commands.as_slice(),
            [OperatorCommand::AcceptRenegotiation { call_id, body }]
                if call_id == "call-a" && body == sdp()
        ));
        assert_eq!(bridge.confirmed_call_count(), 1);
    }

    #[test]
    fn rejected_asterisk_reinvite_keeps_confirmed_call() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let reinvite = String::from_utf8(invite())
            .unwrap()
            .replace("CSeq: 1 INVITE", "CSeq: 2 INVITE");
        bridge.handle_asterisk(reinvite.as_bytes()).unwrap();
        let rejected = bridge
            .handle_operator_event(OperatorEvent::Rejected {
                call_id: "call-a".into(),
                status: 488,
            })
            .unwrap();
        assert!(rejected.asterisk_frames[0].starts_with(b"SIP/2.0 488"));
        assert_eq!(bridge.confirmed_call_count(), 1);
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
            .handle_operator_event(OperatorEvent::Incoming {
                call_id: "ims-call-a".into(),
                caller: "sip:+8613800@ims.example".into(),
                body: sdp().to_vec(),
            })
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

    #[test]
    fn restricted_operator_identity_reaches_asterisk_as_anonymous_only() {
        let incoming = b"INVITE sip:user@example SIP/2.0\r\nPrivacy: id\r\nP-Asserted-Identity: <sip:+15551234567@example>\r\nFrom: <sip:anonymous@anonymous.invalid>;tag=remote\r\n\r\n";
        let caller = crate::connectivity::core::supplementary::resolve_caller_identity(incoming)
            .uri
            .unwrap_or_else(|| "sip:anonymous@anonymous.invalid".to_string());
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven)
        .with_asterisk_target("sip:6108@192.0.2.20:8060");

        let output = bridge
            .handle_operator_event(OperatorEvent::Incoming {
                call_id: "private-ims-call".into(),
                caller,
                body: sdp().to_vec(),
            })
            .unwrap();
        let invite = String::from_utf8_lossy(&output.asterisk_frames[0]);
        assert!(invite.contains("From: <sip:anonymous@anonymous.invalid>;tag="));
        assert!(!invite.contains("+15551234567"));
        assert!(!invite.contains("P-Asserted-Identity"));
    }

    #[test]
    fn rejected_operator_incoming_call_is_acked_and_removed() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven)
        .with_asterisk_target("sip:6108@192.0.2.20:8060");
        let started = bridge
            .handle_operator_event(OperatorEvent::Incoming {
                call_id: "ims-call-rejected".into(),
                caller: "sip:+8613800@ims.example".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let invite = &started.asterisk_frames[0];
        let response = format!(
            "SIP/2.0 486 Busy Here\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=pbx-busy\r\nCall-ID: {}\r\nCSeq: {}\r\nContent-Length: 0\r\n\r\n",
            sip_frame::header_value(invite, "Via").unwrap(),
            sip_frame::header_value(invite, "From").unwrap(),
            sip_frame::header_value(invite, "To").unwrap(),
            sip_frame::header_value(invite, "Call-ID").unwrap(),
            sip_frame::header_value(invite, "CSeq").unwrap(),
        );
        let rejected = bridge.handle_asterisk(response.as_bytes()).unwrap();
        assert!(rejected.asterisk_frames[0].starts_with(b"ACK "));
        assert!(matches!(
            rejected.operator_commands.as_slice(),
            [OperatorCommand::RejectCall { call_id, status: 486 }]
                if call_id == "ims-call-rejected"
        ));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn rejected_operator_reinvite_is_acked_and_keeps_confirmed_call() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let started = bridge
            .handle_operator_event(OperatorEvent::Renegotiate {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let reinvite = &started.asterisk_frames[0];
        let response = format!(
            "SIP/2.0 488 Not Acceptable Here\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=asterisk-a\r\nCall-ID: {}\r\nCSeq: {}\r\nContent-Length: 0\r\n\r\n",
            sip_frame::header_value(reinvite, "Via").unwrap(),
            sip_frame::header_value(reinvite, "From").unwrap(),
            sip_frame::header_value(reinvite, "To").unwrap(),
            sip_frame::header_value(reinvite, "Call-ID").unwrap(),
            sip_frame::header_value(reinvite, "CSeq").unwrap(),
        );
        let rejected = bridge.handle_asterisk(response.as_bytes()).unwrap();
        assert!(rejected.asterisk_frames[0].starts_with(b"ACK "));
        assert!(matches!(
            rejected.operator_commands.as_slice(),
            [OperatorCommand::RejectRenegotiation { call_id, status: 488 }]
                if call_id == "call-a"
        ));
        assert_eq!(bridge.confirmed_call_count(), 1);
    }

    #[test]
    fn operator_cancel_terminates_pending_asterisk_invite() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_asterisk_target("sip:6108@192.0.2.20:8060");
        bridge
            .handle_operator_event(OperatorEvent::Incoming {
                call_id: "ims-call-cancel".into(),
                caller: "sip:+8613800@ims.example".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let output = bridge
            .handle_operator_event(OperatorEvent::Cancelled {
                call_id: "ims-call-cancel".into(),
            })
            .unwrap();
        let cancel = String::from_utf8_lossy(&output.asterisk_frames[0]);
        assert!(cancel.starts_with("CANCEL sip:6108@192.0.2.20:8060 SIP/2.0"));
        assert!(cancel.contains("CSeq: 1 CANCEL\r\n"));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn operator_end_cancels_pending_asterisk_invite() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_asterisk_target("sip:6108@192.0.2.20:8060");
        bridge
            .handle_operator_event(OperatorEvent::Incoming {
                call_id: "ims-call-ended".into(),
                caller: "sip:+8613800@ims.example".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let output = bridge
            .handle_operator_event(OperatorEvent::Ended {
                call_id: "ims-call-ended".into(),
            })
            .unwrap();
        assert_eq!(output.asterisk_frames.len(), 1);
        assert!(output.asterisk_frames[0].starts_with(b"CANCEL "));
        assert_eq!(bridge.active_call_count(), 0);
    }

    #[test]
    fn confirmed_call_forwards_sip_info_dtmf() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let body = b"Signal=5\r\nDuration=240\r\n";
        let info = format!(
            "INFO sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKinfo\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 2 INFO\r\nContent-Type: application/dtmf-relay\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body),
        );
        let output = bridge.handle_asterisk(info.as_bytes()).unwrap();
        assert!(output.asterisk_frames[0].starts_with(b"SIP/2.0 200 OK"));
        assert_eq!(
            output.operator_commands,
            vec![OperatorCommand::SendDtmf {
                call_id: "call-a".into(),
                signal: DtmfSignal {
                    digit: '5',
                    duration_ms: 240,
                    source: DtmfSource::SipInfo,
                },
            }]
        );
    }

    #[test]
    fn confirmed_call_forwards_operator_dtmf_to_asterisk_info() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();

        let output = bridge
            .handle_operator_event(OperatorEvent::Dtmf {
                call_id: "call-a".into(),
                signal: DtmfSignal {
                    digit: '8',
                    duration_ms: 180,
                    source: DtmfSource::SipInfo,
                },
            })
            .unwrap();
        assert_eq!(output.asterisk_frames.len(), 1);
        let info = &output.asterisk_frames[0];
        assert!(info.starts_with(b"INFO "));
        assert_eq!(
            sip_frame::header_value(info, "Content-Type").as_deref(),
            Some("application/dtmf-relay")
        );
        assert_eq!(sip_frame::body(info), b"Signal=8\r\nDuration=180\r\n");
    }

    #[test]
    fn confirmed_call_bridges_refer_response_and_notify_subscription() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();

        let refer = b"REFER sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKrefer\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 2 REFER\r\nRefer-To: <sip:+15551234567@pbx>\r\nContent-Length: 0\r\n\r\n";
        let requested = bridge.handle_asterisk(refer).unwrap();
        assert!(requested.asterisk_frames.is_empty());
        assert_eq!(
            requested.operator_commands,
            vec![OperatorCommand::TransferCall {
                call_id: "call-a".into(),
                refer_to: "sip:+15551234567@pbx".into(),
            }]
        );

        let accepted = bridge
            .handle_operator_event(OperatorEvent::TransferResponse {
                call_id: "call-a".into(),
                status: 202,
            })
            .unwrap();
        assert_eq!(accepted.asterisk_frames.len(), 1);
        assert!(accepted.asterisk_frames[0].starts_with(b"SIP/2.0 202 Accepted"));
        assert_eq!(
            sip_frame::header_value(&accepted.asterisk_frames[0], "CSeq").as_deref(),
            Some("2 REFER")
        );

        let progress = bridge
            .handle_operator_event(OperatorEvent::TransferNotify {
                call_id: "call-a".into(),
                notification: ReferNotification {
                    subscription_state: ReferSubscriptionState::Active,
                    sip_status: 180,
                    transfer_state: DialogTransferState::Trying,
                    event_id: Some(2),
                },
            })
            .unwrap();
        assert_eq!(progress.asterisk_frames.len(), 1);
        let notify = &progress.asterisk_frames[0];
        assert!(notify.starts_with(b"NOTIFY sip:6108@pbx SIP/2.0"));
        assert_eq!(
            sip_frame::header_value(notify, "Event").as_deref(),
            Some("refer;id=2")
        );
        assert_eq!(
            sip_frame::header_value(notify, "Subscription-State").as_deref(),
            Some("active")
        );
        assert_eq!(sip_frame::body(notify), b"SIP/2.0 180 Ringing\r\n");

        let completed = bridge
            .handle_operator_event(OperatorEvent::TransferNotify {
                call_id: "call-a".into(),
                notification: ReferNotification {
                    subscription_state: ReferSubscriptionState::Terminated,
                    sip_status: 200,
                    transfer_state: DialogTransferState::Succeeded,
                    event_id: Some(2),
                },
            })
            .unwrap();
        assert_eq!(
            sip_frame::header_value(&completed.asterisk_frames[0], "Subscription-State").as_deref(),
            Some("terminated;reason=noresource")
        );

        let attended = b"REFER sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKattended\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 3 REFER\r\nRefer-To: <sip:6109@pbx?Replaces=call-b%3Bto-tag%3Da%3Bfrom-tag%3Db>\r\nContent-Length: 0\r\n\r\n";
        let rejected = bridge.handle_asterisk(attended).unwrap();
        assert!(rejected.operator_commands.is_empty());
        assert!(rejected.asterisk_frames[0].starts_with(b"SIP/2.0 501 Not Implemented"));
    }

    #[test]
    fn second_dialog_provisional_and_busy_do_not_disturb_first_call() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        let early = bridge
            .handle_operator_event(OperatorEvent::Provisional {
                call_id: "call-a".into(),
                status: 183,
                body: Some(sdp().to_vec()),
            })
            .unwrap();
        assert!(early.asterisk_frames[0].starts_with(b"SIP/2.0 183 Session Progress"));
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();

        let second = String::from_utf8(invite())
            .unwrap()
            .replace("Call-ID: call-a", "Call-ID: call-b")
            .replace("tag=asterisk-a", "tag=asterisk-b");
        let started = bridge.handle_asterisk(second.as_bytes()).unwrap();
        assert!(matches!(
            started.operator_commands.as_slice(),
            [OperatorCommand::StartCall { call_id, .. }] if call_id == "call-b"
        ));
        let ringing = bridge
            .handle_operator_event(OperatorEvent::Provisional {
                call_id: "call-b".into(),
                status: 180,
                body: None,
            })
            .unwrap();
        assert!(ringing.asterisk_frames[0].starts_with(b"SIP/2.0 180 Ringing"));
        let busy = bridge
            .handle_operator_event(OperatorEvent::Rejected {
                call_id: "call-b".into(),
                status: 486,
            })
            .unwrap();
        assert!(busy.asterisk_frames[0].starts_with(b"SIP/2.0 486"));
        assert!(bridge.has_call("call-a"));
        assert!(!bridge.has_call("call-b"));
        assert_eq!(bridge.confirmed_call_count(), 1);
    }

    #[test]
    fn invalid_dtmf_info_is_rejected_without_operator_command() {
        let mut bridge = TrunkBridge::new(
            SocketAddr::from((Ipv4Addr::new(192, 0, 2, 30), 5062)),
            "sip:41000@192.0.2.30:5062",
        )
        .with_operator(OperatorAvailability::EventDriven);
        bridge.handle_asterisk(&invite()).unwrap();
        bridge
            .handle_operator_event(OperatorEvent::Answered {
                call_id: "call-a".into(),
                body: sdp().to_vec(),
            })
            .unwrap();
        let ack = b"ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n";
        bridge.handle_asterisk(ack).unwrap();
        let info = b"INFO sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bKinfo\r\nFrom: <sip:6108@pbx>;tag=asterisk-a\r\nTo: <sip:41000@simadmin>;tag=local\r\nCall-ID: call-a\r\nCSeq: 2 INFO\r\nContent-Type: application/dtmf\r\nContent-Length: 1\r\n\r\nZ";
        let output = bridge.handle_asterisk(info).unwrap();
        assert!(output.asterisk_frames[0].starts_with(b"SIP/2.0 400 Bad Request"));
        assert!(output.operator_commands.is_empty());
    }
}
