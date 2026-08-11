#![allow(dead_code)]

use std::collections::VecDeque;

use aes::{Aes128, Aes256};
use cbc::cipher::{block_padding::NoPadding, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::Serialize;

use super::{ike_keys::ChildSaSecretPair, profiles::CarrierProfile};

const DEFAULT_ANTI_REPLAY_WINDOW: u16 = 64;
const DEFAULT_OUTER_MTU: u16 = 1500;
const ESP_HEADER_BYTES: u16 = 8;
const UDP_HEADER_BYTES: u16 = 8;
const IPV4_HEADER_BYTES: u16 = 20;
const AES_CBC_IV_BYTES: u16 = 16;
const ESP_TRAILER_BUDGET_BYTES: u16 = 17;
const DEFAULT_INTEGRITY_CHECK_BYTES: u16 = 16;
const INNER_QUEUE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataplanePhase {
    Idle,
    ChildSaNegotiated,
    EspSecretsReady,
    InnerStackReady,
    Forwarding,
    Rekey,
    Teardown,
    Failed,
}

impl DataplanePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::ChildSaNegotiated => "child_sa_negotiated",
            Self::EspSecretsReady => "esp_secrets_ready",
            Self::InnerStackReady => "inner_stack_ready",
            Self::Forwarding => "forwarding",
            Self::Rekey => "rekey",
            Self::Teardown => "teardown",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataplaneEspProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub encapsulation: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrafficSelectorPlan {
    pub local_selector: &'static str,
    pub remote_selector: &'static str,
    pub address_assignment: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmoltcpGatewayPlan {
    pub stack: &'static str,
    pub gateway_mode: &'static str,
    pub ip_stack: &'static str,
    pub tcp_enabled: bool,
    pub udp_enabled: bool,
    pub icmp_enabled: bool,
    pub socket_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MtuPlan {
    pub outer_mtu: u16,
    pub estimated_overhead_bytes: u16,
    pub inner_mtu: u16,
    pub ipv4_header_bytes: u16,
    pub udp_header_bytes: u16,
    pub esp_header_bytes: u16,
    pub iv_bytes: u16,
    pub trailer_budget_bytes: u16,
    pub integrity_check_bytes: u16,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataplanePlan {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub outer_encapsulation: &'static str,
    pub nat_t_port: u16,
    pub nat_keepalive_seconds: u16,
    pub anti_replay_window: u16,
    pub mtu_strategy: &'static str,
    pub mtu: MtuPlan,
    pub traffic_selectors: TrafficSelectorPlan,
    pub smoltcp: SmoltcpGatewayPlan,
    pub esp_proposals: Vec<DataplaneEspProposalPlan>,
    pub plaintext_capture_policy: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataplaneRuntimeState {
    pub phase: &'static str,
    pub tunnel_trace_id: String,
    pub selected_esp_proposal: Option<String>,
    pub inner_ip_ready: bool,
    pub replay_window_ready: bool,
    pub packets_in: u64,
    pub packets_out: u64,
    pub last_error: Option<String>,
}

impl Default for DataplaneRuntimeState {
    fn default() -> Self {
        Self {
            phase: DataplanePhase::Idle.as_str(),
            tunnel_trace_id: String::new(),
            selected_esp_proposal: None,
            inner_ip_ready: false,
            replay_window_ready: false,
            packets_in: 0,
            packets_out: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AntiReplayWindowSnapshot {
    pub window_size: u16,
    pub highest_sequence: u64,
    pub tracked_slots: u16,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AntiReplayDecisionKind {
    AcceptedNewHighest,
    AcceptedWithinWindow,
    RejectedDuplicate,
    RejectedTooOld,
    RejectedInvalidSequence,
}

impl AntiReplayDecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcceptedNewHighest => "accepted_new_highest",
            Self::AcceptedWithinWindow => "accepted_within_window",
            Self::RejectedDuplicate => "rejected_duplicate",
            Self::RejectedTooOld => "rejected_too_old",
            Self::RejectedInvalidSequence => "rejected_invalid_sequence",
        }
    }

    pub fn accepted(self) -> bool {
        matches!(self, Self::AcceptedNewHighest | Self::AcceptedWithinWindow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AntiReplayDecision {
    pub decision: &'static str,
    pub accepted: bool,
    pub highest_sequence: u64,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntiReplayWindow {
    window_size: u16,
    highest_sequence: u64,
    bitmap: u128,
}

impl AntiReplayWindow {
    pub fn new(window_size: u16) -> Self {
        Self {
            window_size: window_size.clamp(1, 128),
            highest_sequence: 0,
            bitmap: 0,
        }
    }

    pub fn snapshot(&self) -> AntiReplayWindowSnapshot {
        AntiReplayWindowSnapshot {
            window_size: self.window_size,
            highest_sequence: self.highest_sequence,
            tracked_slots: self.bitmap.count_ones() as u16,
            sensitive_values_policy: "only_sequence_window_metadata_serialized",
        }
    }

    pub fn accept(&mut self, sequence_number: u64) -> AntiReplayDecision {
        if sequence_number == 0 {
            return self.decision(AntiReplayDecisionKind::RejectedInvalidSequence);
        }

        if self.highest_sequence == 0 || sequence_number > self.highest_sequence {
            let delta = sequence_number.saturating_sub(self.highest_sequence);
            self.bitmap = if delta >= 128 {
                1
            } else {
                (self.bitmap << delta) | 1
            } & self.window_mask();
            self.highest_sequence = sequence_number;
            return self.decision(AntiReplayDecisionKind::AcceptedNewHighest);
        }

        let offset = self.highest_sequence - sequence_number;
        if offset >= u64::from(self.window_size) {
            return self.decision(AntiReplayDecisionKind::RejectedTooOld);
        }

        let mask = 1u128 << offset;
        if self.bitmap & mask != 0 {
            return self.decision(AntiReplayDecisionKind::RejectedDuplicate);
        }

        self.bitmap |= mask;
        self.decision(AntiReplayDecisionKind::AcceptedWithinWindow)
    }

    fn decision(&self, kind: AntiReplayDecisionKind) -> AntiReplayDecision {
        AntiReplayDecision {
            decision: kind.as_str(),
            accepted: kind.accepted(),
            highest_sequence: self.highest_sequence,
            sensitive_values_policy: "esp_sequence_metadata_only",
        }
    }

    fn window_mask(&self) -> u128 {
        if self.window_size == 128 {
            u128::MAX
        } else {
            (1u128 << self.window_size) - 1
        }
    }
}

impl Default for AntiReplayWindow {
    fn default() -> Self {
        Self::new(DEFAULT_ANTI_REPLAY_WINDOW)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspPacketMetadata {
    pub sa_identifier_present: bool,
    pub sequence_number: u64,
    pub protected_bytes: usize,
    pub outer_frame_bytes: usize,
    pub header_bytes: usize,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NattPacketKind {
    EspInUdp4500,
    IkeNonEspMarker,
    NatKeepalive,
}

impl NattPacketKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EspInUdp4500 => "esp_in_udp_4500",
            Self::IkeNonEspMarker => "ike_non_esp_marker",
            Self::NatKeepalive => "nat_keepalive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NattPacketSummary {
    pub kind: &'static str,
    pub udp_port: u16,
    pub wire_bytes: usize,
    pub esp: Option<EspPacketMetadata>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MtuDecision {
    pub decision: &'static str,
    pub accepted: bool,
    pub inner_packet_bytes: usize,
    pub estimated_outer_frame_bytes: usize,
    pub outer_mtu: u16,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InnerPacketMetadata {
    pub packet_id: u64,
    pub direction: &'static str,
    pub ip_version: &'static str,
    pub packet_bytes: usize,
    pub accepted: bool,
    pub drop_reason: Option<&'static str>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InnerGatewayPublicState {
    pub adapter: &'static str,
    pub ip_stack: &'static str,
    pub queue_capacity: usize,
    pub queued_packets: usize,
    pub packets_to_esp: u64,
    pub packets_from_esp: u64,
    pub last_packet: Option<InnerPacketMetadata>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaPairPublicState {
    pub inbound_sa_identifier_present: bool,
    pub outbound_sa_identifier_present: bool,
    pub outbound_sequence_allocated: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspFrameDecision {
    pub direction: &'static str,
    pub decision: &'static str,
    pub accepted: bool,
    pub sequence_number: Option<u64>,
    pub outer_frame_bytes: usize,
    pub natt: Option<NattPacketSummary>,
    pub mtu: Option<MtuDecision>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildSaPublicState {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub phase: &'static str,
    pub selected_esp_proposal: Option<&'static str>,
    pub inbound_sa_identifier_present: bool,
    pub outbound_sa_identifier_present: bool,
    pub sa_pair: SaPairPublicState,
    pub replay_window: AntiReplayWindowSnapshot,
    pub mtu: MtuPlan,
    pub mtu_drops: u64,
    pub smoltcp: SmoltcpGatewayPlan,
    pub inner_gateway: InnerGatewayPublicState,
    pub packets_in: u64,
    pub packets_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub last_frame_decision: Option<EspFrameDecision>,
    pub last_error: Option<String>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataplaneStateError {
    EmptyEspProposals,
    InvalidSaIdentifier,
    InvalidSelectedEspProposal,
    EspPacketTooShort,
    InvalidPhase {
        expected: &'static str,
        actual: &'static str,
    },
    SequenceExhausted,
    InnerPacketTooLarge {
        inner_packet_bytes: usize,
        inner_mtu: u16,
    },
    InnerQueueFull,
    EspIntegrityMismatch,
    EspInvalidPadding,
    EspUnsupportedCipher,
    EspUnsupportedIntegrity,
    EspRandomFailed,
}

impl std::fmt::Display for DataplaneStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyEspProposals => write!(f, "profile has no ESP proposals"),
            Self::InvalidSaIdentifier => write!(f, "invalid CHILD_SA identifier"),
            Self::InvalidSelectedEspProposal => {
                write!(f, "selected ESP proposal is not in the profile")
            }
            Self::EspPacketTooShort => write!(f, "ESP frame is too short"),
            Self::InvalidPhase { expected, actual } => {
                write!(
                    f,
                    "invalid dataplane phase expected={expected} actual={actual}"
                )
            }
            Self::SequenceExhausted => write!(f, "outbound ESP sequence exhausted"),
            Self::InnerPacketTooLarge {
                inner_packet_bytes,
                inner_mtu,
            } => write!(
                f,
                "inner packet exceeds planned MTU bytes={inner_packet_bytes} inner_mtu={inner_mtu}"
            ),
            Self::InnerQueueFull => write!(f, "inner gateway queue is full"),
            Self::EspIntegrityMismatch => write!(f, "ESP integrity check failed"),
            Self::EspInvalidPadding => write!(f, "ESP padding is invalid"),
            Self::EspUnsupportedCipher => write!(f, "ESP cipher is unsupported"),
            Self::EspUnsupportedIntegrity => write!(f, "ESP integrity algorithm is unsupported"),
            Self::EspRandomFailed => write!(f, "ESP random generation failed"),
        }
    }
}

impl std::error::Error for DataplaneStateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SaDirectionState {
    sa_identifier: u32,
    next_sequence: u64,
    packets: u64,
    bytes: u64,
}

impl SaDirectionState {
    fn new(sa_identifier: u32) -> Result<Self, DataplaneStateError> {
        if sa_identifier == 0 {
            return Err(DataplaneStateError::InvalidSaIdentifier);
        }
        Ok(Self {
            sa_identifier,
            next_sequence: 1,
            packets: 0,
            bytes: 0,
        })
    }

    fn allocate_sequence(&mut self) -> Result<u64, DataplaneStateError> {
        let sequence = self.next_sequence;
        if sequence > u64::from(u32::MAX) {
            return Err(DataplaneStateError::SequenceExhausted);
        }
        self.next_sequence += 1;
        Ok(sequence)
    }

    fn record(&mut self, bytes: usize) {
        self.packets = self.packets.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes as u64);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InnerGatewayState {
    ip_stack: &'static str,
    queue_capacity: usize,
    next_packet_id: u64,
    packets_to_esp: u64,
    packets_from_esp: u64,
    queue: VecDeque<InnerPacketMetadata>,
    last_packet: Option<InnerPacketMetadata>,
}

impl InnerGatewayState {
    fn new(ip_stack: &'static str) -> Self {
        Self {
            ip_stack,
            queue_capacity: INNER_QUEUE_CAPACITY,
            next_packet_id: 1,
            packets_to_esp: 0,
            packets_from_esp: 0,
            queue: VecDeque::with_capacity(INNER_QUEUE_CAPACITY),
            last_packet: None,
        }
    }

    fn snapshot(&self) -> InnerGatewayPublicState {
        InnerGatewayPublicState {
            adapter: "smoltcp_virtual_device",
            ip_stack: self.ip_stack,
            queue_capacity: self.queue_capacity,
            queued_packets: self.queue.len(),
            packets_to_esp: self.packets_to_esp,
            packets_from_esp: self.packets_from_esp,
            last_packet: self.last_packet.clone(),
            sensitive_values_policy: "inner_packet_metadata_only_no_payload_serialized",
        }
    }

    fn enqueue_outbound(
        &mut self,
        packet_bytes: &[u8],
    ) -> Result<InnerPacketMetadata, DataplaneStateError> {
        if self.queue.len() >= self.queue_capacity {
            return Err(DataplaneStateError::InnerQueueFull);
        }

        let metadata = InnerPacketMetadata {
            packet_id: self.next_packet_id,
            direction: "outbound",
            ip_version: classify_ip_version(packet_bytes),
            packet_bytes: packet_bytes.len(),
            accepted: true,
            drop_reason: None,
            sensitive_values_policy: "packet_bytes_not_serialized",
        };
        self.next_packet_id = self.next_packet_id.saturating_add(1);
        self.packets_to_esp = self.packets_to_esp.saturating_add(1);
        self.queue.push_back(metadata.clone());
        self.last_packet = Some(metadata.clone());
        Ok(metadata)
    }

    fn record_inbound(&mut self, protected_bytes: usize) -> InnerPacketMetadata {
        let metadata = InnerPacketMetadata {
            packet_id: self.next_packet_id,
            direction: "inbound",
            ip_version: "protected_inner_packet",
            packet_bytes: protected_bytes,
            accepted: true,
            drop_reason: None,
            sensitive_values_policy: "inner_packet_not_decrypted_in_m5",
        };
        self.next_packet_id = self.next_packet_id.saturating_add(1);
        self.packets_from_esp = self.packets_from_esp.saturating_add(1);
        self.last_packet = Some(metadata.clone());
        metadata
    }

    fn record_drop(&mut self, packet_bytes: &[u8], reason: &'static str) -> InnerPacketMetadata {
        let metadata = InnerPacketMetadata {
            packet_id: self.next_packet_id,
            direction: "outbound",
            ip_version: classify_ip_version(packet_bytes),
            packet_bytes: packet_bytes.len(),
            accepted: false,
            drop_reason: Some(reason),
            sensitive_values_policy: "packet_bytes_not_serialized",
        };
        self.next_packet_id = self.next_packet_id.saturating_add(1);
        self.last_packet = Some(metadata.clone());
        metadata
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSaStateMachine {
    profile: &'static CarrierProfile,
    phase: DataplanePhase,
    selected_esp_proposal: Option<DataplaneEspProposalPlan>,
    inbound_sa: Option<SaDirectionState>,
    outbound_sa: Option<SaDirectionState>,
    replay_window: AntiReplayWindow,
    mtu: MtuPlan,
    mtu_drops: u64,
    inner_gateway: InnerGatewayState,
    last_frame_decision: Option<EspFrameDecision>,
    last_error: Option<String>,
}

impl ChildSaStateMachine {
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self {
            profile,
            phase: DataplanePhase::Idle,
            selected_esp_proposal: None,
            inbound_sa: None,
            outbound_sa: None,
            replay_window: AntiReplayWindow::new(DEFAULT_ANTI_REPLAY_WINDOW),
            mtu: build_mtu_plan(DEFAULT_OUTER_MTU),
            mtu_drops: 0,
            inner_gateway: InnerGatewayState::new(profile.epdg.ip_stack),
            last_frame_decision: None,
            last_error: None,
        }
    }

    pub fn snapshot(&self) -> ChildSaPublicState {
        let plan = build_dataplane_plan(self.profile);
        let inbound = self.inbound_sa.as_ref();
        let outbound = self.outbound_sa.as_ref();
        let packets_in = inbound.map(|sa| sa.packets).unwrap_or_default();
        let packets_out = outbound.map(|sa| sa.packets).unwrap_or_default();
        let bytes_in = inbound.map(|sa| sa.bytes).unwrap_or_default();
        let bytes_out = outbound.map(|sa| sa.bytes).unwrap_or_default();
        ChildSaPublicState {
            profile_id: self.profile.meta.profile_id,
            plmn: self.profile.meta.plmn,
            phase: self.phase.as_str(),
            selected_esp_proposal: self
                .selected_esp_proposal
                .as_ref()
                .map(|proposal| proposal.proposal),
            inbound_sa_identifier_present: inbound.is_some(),
            outbound_sa_identifier_present: outbound.is_some(),
            sa_pair: SaPairPublicState {
                inbound_sa_identifier_present: inbound.is_some(),
                outbound_sa_identifier_present: outbound.is_some(),
                outbound_sequence_allocated: outbound
                    .map(|sa| sa.next_sequence.saturating_sub(1))
                    .unwrap_or_default(),
                packets_in,
                packets_out,
                bytes_in,
                bytes_out,
                sensitive_values_policy: "sa_identifiers_and_key_material_not_serialized",
            },
            replay_window: self.replay_window.snapshot(),
            mtu: self.mtu.clone(),
            mtu_drops: self.mtu_drops,
            smoltcp: plan.smoltcp,
            inner_gateway: self.inner_gateway.snapshot(),
            packets_in,
            packets_out,
            bytes_in,
            bytes_out,
            last_frame_decision: self.last_frame_decision.clone(),
            last_error: self.last_error.clone(),
            sensitive_values_policy: "sa_identifiers_secrets_and_frame_bodies_not_serialized",
        }
    }

    pub fn negotiate_child_sa(
        &mut self,
        inbound_sa_identifier: u32,
        outbound_sa_identifier: u32,
    ) -> Result<(), DataplaneStateError> {
        let proposal = self
            .profile
            .ikev2
            .esp_proposals
            .first()
            .copied()
            .ok_or(DataplaneStateError::EmptyEspProposals)?;
        self.negotiate_child_sa_with_profile_proposal(
            inbound_sa_identifier,
            outbound_sa_identifier,
            proposal,
        )
    }

    pub fn negotiate_child_sa_with_profile_proposal(
        &mut self,
        inbound_sa_identifier: u32,
        outbound_sa_identifier: u32,
        selected_profile_proposal: &'static str,
    ) -> Result<(), DataplaneStateError> {
        self.require_phase(DataplanePhase::Idle)?;
        let inbound_sa = SaDirectionState::new(inbound_sa_identifier).inspect_err(|err| {
            self.phase = DataplanePhase::Failed;
            self.last_error = Some(err.to_string());
        })?;
        let outbound_sa = SaDirectionState::new(outbound_sa_identifier).inspect_err(|err| {
            self.phase = DataplanePhase::Failed;
            self.last_error = Some(err.to_string());
        })?;
        if !self
            .profile
            .ikev2
            .esp_proposals
            .contains(&selected_profile_proposal)
        {
            self.phase = DataplanePhase::Failed;
            self.last_error = Some("selected ESP proposal is not in the profile".to_string());
            return Err(DataplaneStateError::InvalidSelectedEspProposal);
        }
        let proposal = parse_esp_proposal(selected_profile_proposal);

        self.selected_esp_proposal = Some(proposal);
        self.inbound_sa = Some(inbound_sa);
        self.outbound_sa = Some(outbound_sa);
        self.phase = DataplanePhase::ChildSaNegotiated;
        Ok(())
    }

    pub fn mark_esp_secrets_ready(&mut self) -> Result<(), DataplaneStateError> {
        self.require_phase(DataplanePhase::ChildSaNegotiated)?;
        self.phase = DataplanePhase::EspSecretsReady;
        Ok(())
    }

    pub fn mark_inner_stack_ready(&mut self) -> Result<(), DataplaneStateError> {
        self.require_phase(DataplanePhase::EspSecretsReady)?;
        self.phase = DataplanePhase::InnerStackReady;
        Ok(())
    }

    pub fn begin_rekey(&mut self) -> Result<(), DataplaneStateError> {
        self.require_ready_for_frames()?;
        self.phase = DataplanePhase::Rekey;
        Ok(())
    }

    pub fn teardown(&mut self) {
        self.phase = DataplanePhase::Teardown;
        self.inbound_sa = None;
        self.outbound_sa = None;
        self.selected_esp_proposal = None;
        self.inner_gateway.queue.clear();
    }

    pub fn record_inbound_esp_frame(
        &mut self,
        sequence_number: u64,
        outer_frame_bytes: usize,
    ) -> Result<EspFrameDecision, DataplaneStateError> {
        self.require_ready_for_frames()?;
        let replay = self.replay_window.accept(sequence_number);
        let decision = EspFrameDecision {
            direction: "inbound",
            decision: replay.decision,
            accepted: replay.accepted,
            sequence_number: Some(sequence_number),
            outer_frame_bytes,
            natt: None,
            mtu: None,
            sensitive_values_policy: "frame_body_not_serialized",
        };
        if replay.accepted {
            if let Some(inbound) = self.inbound_sa.as_mut() {
                inbound.record(outer_frame_bytes);
            }
            self.phase = DataplanePhase::Forwarding;
        }
        self.last_frame_decision = Some(decision.clone());
        Ok(decision)
    }

    pub fn record_inbound_nat_t_packet(
        &mut self,
        wire_bytes: &[u8],
    ) -> Result<EspFrameDecision, DataplaneStateError> {
        self.require_ready_for_frames()?;
        let natt = parse_nat_t_packet(wire_bytes)?;
        let Some(esp) = natt.esp.clone() else {
            let decision = EspFrameDecision {
                direction: "inbound",
                decision: natt.kind,
                accepted: false,
                sequence_number: None,
                outer_frame_bytes: wire_bytes.len(),
                natt: Some(natt),
                mtu: None,
                sensitive_values_policy: "control_or_keepalive_packet_metadata_only",
            };
            self.last_frame_decision = Some(decision.clone());
            return Ok(decision);
        };

        let replay = self.replay_window.accept(esp.sequence_number);
        let decision = EspFrameDecision {
            direction: "inbound",
            decision: replay.decision,
            accepted: replay.accepted,
            sequence_number: Some(esp.sequence_number),
            outer_frame_bytes: wire_bytes.len(),
            natt: Some(natt),
            mtu: None,
            sensitive_values_policy: "frame_body_not_serialized",
        };
        if replay.accepted {
            if let Some(inbound) = self.inbound_sa.as_mut() {
                inbound.record(wire_bytes.len());
            }
            self.inner_gateway.record_inbound(esp.protected_bytes);
            self.phase = DataplanePhase::Forwarding;
        }
        self.last_frame_decision = Some(decision.clone());
        Ok(decision)
    }

    pub fn record_outbound_esp_frame(
        &mut self,
        outer_frame_bytes: usize,
    ) -> Result<EspFrameDecision, DataplaneStateError> {
        self.require_ready_for_frames()?;
        let sequence_number = self
            .outbound_sa
            .as_mut()
            .ok_or(DataplaneStateError::InvalidSaIdentifier)?
            .allocate_sequence()?;
        self.record_outbound_wire_frame(sequence_number, outer_frame_bytes, None, None)
    }

    pub fn record_outbound_inner_packet(
        &mut self,
        inner_packet: &[u8],
    ) -> Result<EspFrameDecision, DataplaneStateError> {
        self.require_ready_for_frames()?;
        let mtu = self.evaluate_mtu(inner_packet.len());
        if !mtu.accepted {
            self.mtu_drops = self.mtu_drops.saturating_add(1);
            self.inner_gateway.record_drop(inner_packet, "mtu_exceeded");
            let decision = EspFrameDecision {
                direction: "outbound",
                decision: "mtu_drop",
                accepted: false,
                sequence_number: None,
                outer_frame_bytes: mtu.estimated_outer_frame_bytes,
                natt: None,
                mtu: Some(mtu),
                sensitive_values_policy: "inner_packet_not_serialized",
            };
            self.last_frame_decision = Some(decision.clone());
            return Ok(decision);
        }

        self.inner_gateway.enqueue_outbound(inner_packet)?;
        let outbound = self
            .outbound_sa
            .as_mut()
            .ok_or(DataplaneStateError::InvalidSaIdentifier)?;
        let sequence_number = outbound.allocate_sequence()?;
        let esp_frame = build_esp_frame_metadata(
            outbound.sa_identifier,
            sequence_number,
            protected_len_for_inner_packet(inner_packet.len()),
        )?;
        let natt = wrap_esp_metadata_for_nat_t(esp_frame);
        self.record_outbound_wire_frame(sequence_number, natt.wire_bytes, Some(natt), Some(mtu))
    }

    fn record_outbound_wire_frame(
        &mut self,
        sequence_number: u64,
        outer_frame_bytes: usize,
        natt: Option<NattPacketSummary>,
        mtu: Option<MtuDecision>,
    ) -> Result<EspFrameDecision, DataplaneStateError> {
        if let Some(outbound) = self.outbound_sa.as_mut() {
            outbound.record(outer_frame_bytes);
        }
        self.phase = DataplanePhase::Forwarding;
        let decision = EspFrameDecision {
            direction: "outbound",
            decision: "accepted_local_frame",
            accepted: true,
            sequence_number: Some(sequence_number),
            outer_frame_bytes,
            natt,
            mtu,
            sensitive_values_policy: "frame_body_not_serialized",
        };
        self.last_frame_decision = Some(decision.clone());
        Ok(decision)
    }

    fn evaluate_mtu(&self, inner_packet_bytes: usize) -> MtuDecision {
        let estimated_outer_frame_bytes =
            inner_packet_bytes.saturating_add(self.mtu.estimated_overhead_bytes as usize);
        let accepted = estimated_outer_frame_bytes <= usize::from(self.mtu.outer_mtu);
        MtuDecision {
            decision: if accepted {
                "accepted_within_mtu"
            } else {
                "mtu_exceeded"
            },
            accepted,
            inner_packet_bytes,
            estimated_outer_frame_bytes,
            outer_mtu: self.mtu.outer_mtu,
            sensitive_values_policy: "packet_lengths_only",
        }
    }

    fn require_ready_for_frames(&self) -> Result<(), DataplaneStateError> {
        match self.phase {
            DataplanePhase::InnerStackReady | DataplanePhase::Forwarding => Ok(()),
            actual => Err(DataplaneStateError::InvalidPhase {
                expected: "inner_stack_ready_or_forwarding",
                actual: actual.as_str(),
            }),
        }
    }

    fn require_phase(&self, expected: DataplanePhase) -> Result<(), DataplaneStateError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(DataplaneStateError::InvalidPhase {
                expected: expected.as_str(),
                actual: self.phase.as_str(),
            })
        }
    }
}

pub fn build_dataplane_plan(profile: &'static CarrierProfile) -> DataplanePlan {
    DataplanePlan {
        profile_id: profile.meta.profile_id,
        plmn: profile.meta.plmn,
        outer_encapsulation: "esp_in_udp_nat_t",
        nat_t_port: 4500,
        nat_keepalive_seconds: profile.ikev2.nat_keepalive_seconds,
        anti_replay_window: DEFAULT_ANTI_REPLAY_WINDOW,
        mtu_strategy: "inner_path_mtu_with_ipv4_udp_esp_overhead",
        mtu: build_mtu_plan(DEFAULT_OUTER_MTU),
        traffic_selectors: TrafficSelectorPlan {
            local_selector: "assigned_inner_address",
            remote_selector: "ims_services",
            address_assignment: "ikev2_configuration_payload",
        },
        smoltcp: SmoltcpGatewayPlan {
            stack: "smoltcp",
            gateway_mode: "userspace_inner_ip_gateway",
            ip_stack: profile.epdg.ip_stack,
            tcp_enabled: profile.ims.transport == "tcp" || profile.sms.receiver_transport == "tcp",
            udp_enabled: profile.ims.transport == "udp",
            icmp_enabled: false,
            socket_policy: "bounded_per_profile_runtime",
        },
        esp_proposals: profile
            .ikev2
            .esp_proposals
            .iter()
            .map(|proposal| parse_esp_proposal(proposal))
            .collect(),
        plaintext_capture_policy: "disabled_by_default_metadata_only",
        sensitive_values_policy: "secrets_never_serialized",
    }
}

pub fn build_mtu_plan(outer_mtu: u16) -> MtuPlan {
    let estimated_overhead_bytes = IPV4_HEADER_BYTES
        + UDP_HEADER_BYTES
        + ESP_HEADER_BYTES
        + AES_CBC_IV_BYTES
        + ESP_TRAILER_BUDGET_BYTES
        + DEFAULT_INTEGRITY_CHECK_BYTES;
    MtuPlan {
        outer_mtu,
        estimated_overhead_bytes,
        inner_mtu: outer_mtu.saturating_sub(estimated_overhead_bytes),
        ipv4_header_bytes: IPV4_HEADER_BYTES,
        udp_header_bytes: UDP_HEADER_BYTES,
        esp_header_bytes: ESP_HEADER_BYTES,
        iv_bytes: AES_CBC_IV_BYTES,
        trailer_budget_bytes: ESP_TRAILER_BUDGET_BYTES,
        integrity_check_bytes: DEFAULT_INTEGRITY_CHECK_BYTES,
        sensitive_values_policy: "mtu_metadata_only",
    }
}

pub fn parse_esp_frame_metadata(frame: &[u8]) -> Result<EspPacketMetadata, DataplaneStateError> {
    if frame.len() < ESP_HEADER_BYTES as usize {
        return Err(DataplaneStateError::EspPacketTooShort);
    }

    let sequence_number = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as u64;
    Ok(EspPacketMetadata {
        sa_identifier_present: frame[..4] != [0, 0, 0, 0],
        sequence_number,
        protected_bytes: frame.len() - ESP_HEADER_BYTES as usize,
        outer_frame_bytes: frame.len(),
        header_bytes: ESP_HEADER_BYTES as usize,
        sensitive_values_policy: "sa_identifier_and_protected_bytes_not_serialized",
    })
}

pub fn build_esp_frame_metadata(
    sa_identifier: u32,
    sequence_number: u64,
    protected_bytes: usize,
) -> Result<EspPacketMetadata, DataplaneStateError> {
    if sa_identifier == 0 {
        return Err(DataplaneStateError::InvalidSaIdentifier);
    }
    if sequence_number == 0 || sequence_number > u64::from(u32::MAX) {
        return Err(DataplaneStateError::SequenceExhausted);
    }
    Ok(EspPacketMetadata {
        sa_identifier_present: true,
        sequence_number,
        protected_bytes,
        outer_frame_bytes: ESP_HEADER_BYTES as usize + protected_bytes,
        header_bytes: ESP_HEADER_BYTES as usize,
        sensitive_values_policy: "sa_identifier_and_protected_bytes_not_serialized",
    })
}

pub fn build_esp_frame_bytes(
    sa_identifier: u32,
    sequence_number: u64,
    protected_bytes: &[u8],
) -> Result<Vec<u8>, DataplaneStateError> {
    let _metadata =
        build_esp_frame_metadata(sa_identifier, sequence_number, protected_bytes.len())?;
    let mut frame = Vec::with_capacity(ESP_HEADER_BYTES as usize + protected_bytes.len());
    frame.extend_from_slice(&sa_identifier.to_be_bytes());
    frame.extend_from_slice(&(sequence_number as u32).to_be_bytes());
    frame.extend_from_slice(protected_bytes);
    Ok(frame)
}

pub fn parse_nat_t_packet(packet: &[u8]) -> Result<NattPacketSummary, DataplaneStateError> {
    if packet.len() == 1 && packet[0] == 0xff {
        return Ok(NattPacketSummary {
            kind: NattPacketKind::NatKeepalive.as_str(),
            udp_port: 4500,
            wire_bytes: packet.len(),
            esp: None,
            sensitive_values_policy: "nat_keepalive_metadata_only",
        });
    }

    if packet.len() >= 4 && packet[..4] == [0, 0, 0, 0] {
        return Ok(NattPacketSummary {
            kind: NattPacketKind::IkeNonEspMarker.as_str(),
            udp_port: 4500,
            wire_bytes: packet.len(),
            esp: None,
            sensitive_values_policy: "ike_packet_body_not_serialized",
        });
    }

    let esp = parse_esp_frame_metadata(packet)?;
    Ok(NattPacketSummary {
        kind: NattPacketKind::EspInUdp4500.as_str(),
        udp_port: 4500,
        wire_bytes: packet.len(),
        esp: Some(esp),
        sensitive_values_policy: "esp_packet_metadata_only",
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EspProtectedPacketSummary {
    pub sequence_number: u64,
    pub outer_frame_bytes: usize,
    pub protected_bytes: usize,
    pub inner_packet_bytes: usize,
    pub next_header: u8,
    pub icv_bytes: usize,
    pub sensitive_values_policy: &'static str,
}

pub fn protect_inner_packet_for_esp(
    sa_identifier: u32,
    sequence_number: u64,
    inner_packet: &[u8],
    next_header: u8,
    secrets: &ChildSaSecretPair,
) -> Result<(Vec<u8>, EspProtectedPacketSummary), DataplaneStateError> {
    protect_inner_packet_for_esp_with_mode(
        sa_identifier,
        sequence_number,
        inner_packet,
        next_header,
        secrets,
        true,
    )
}

/// Encrypt/authenticate an inner packet into an ESP transport-mode frame.
///
/// `icv_include_iv` selects whether the integrity check value covers the
/// explicit IV in addition to SPI + sequence + ciphertext (RFC 4303 §3.3.2,
/// the standard) or skips the IV (a non-standard variant seen in some IMS
/// stacks). Most P-CSCFs implement the RFC 4303 rule; the alternate is kept
/// as a wire-format candidate for interop.
pub fn protect_inner_packet_for_esp_with_mode(
    sa_identifier: u32,
    sequence_number: u64,
    inner_packet: &[u8],
    next_header: u8,
    secrets: &ChildSaSecretPair,
    icv_include_iv: bool,
) -> Result<(Vec<u8>, EspProtectedPacketSummary), DataplaneStateError> {
    let _metadata = build_esp_frame_metadata(sa_identifier, sequence_number, inner_packet.len())?;
    let plan = secrets.summary();
    let iv = if esp_iv_bytes(plan.encryption)? == 0 {
        Vec::new()
    } else {
        random_esp_iv()?.to_vec()
    };
    let mut plaintext = Vec::from(inner_packet);
    let pad_len = esp_pad_len(plaintext.len(), 16);
    plaintext.extend((1..=pad_len).map(|value| value as u8));
    plaintext.push(pad_len as u8);
    plaintext.push(next_header);
    let ciphertext = if plan.encryption == "null" {
        plaintext.clone()
    } else {
        esp_encrypt_cbc(
            plan.encryption,
            secrets.outbound_encryption.expose_for_protocol(),
            &iv,
            &plaintext,
        )?
    };

    let mut frame = Vec::with_capacity(
        ESP_HEADER_BYTES as usize + iv.len() + ciphertext.len() + plan.integrity_key_bytes,
    );
    frame.extend_from_slice(&sa_identifier.to_be_bytes());
    frame.extend_from_slice(&(sequence_number as u32).to_be_bytes());
    frame.extend_from_slice(&iv);
    frame.extend_from_slice(&ciphertext);
    let icv_input = if icv_include_iv {
        frame.clone()
    } else {
        // SPI | seq | ciphertext -- explicit IV is not covered by the ICV.
        let mut input = Vec::with_capacity(8 + ciphertext.len());
        input.extend_from_slice(&sa_identifier.to_be_bytes());
        input.extend_from_slice(&(sequence_number as u32).to_be_bytes());
        input.extend_from_slice(&ciphertext);
        input
    };
    let icv = esp_integrity_tag(
        plan.integrity,
        secrets.outbound_integrity.expose_for_protocol(),
        &icv_input,
    )?;
    frame.extend_from_slice(&icv);

    Ok((
        frame.clone(),
        EspProtectedPacketSummary {
            sequence_number,
            outer_frame_bytes: frame.len(),
            protected_bytes: frame.len() - ESP_HEADER_BYTES as usize,
            inner_packet_bytes: inner_packet.len(),
            next_header,
            icv_bytes: icv.len(),
            sensitive_values_policy: "packet_lengths_only_no_payload_or_key_material",
        },
    ))
}

pub fn unprotect_inner_packet_from_esp(
    frame: &[u8],
    secrets: &ChildSaSecretPair,
) -> Result<(Vec<u8>, EspProtectedPacketSummary), DataplaneStateError> {
    unprotect_inner_packet_from_esp_with_mode(frame, secrets, true)
}

/// Inbound counterpart of [`protect_inner_packet_for_esp_with_mode`]; the
/// `icv_include_iv` flag must match the mode used by the peer.
pub fn unprotect_inner_packet_from_esp_with_mode(
    frame: &[u8],
    secrets: &ChildSaSecretPair,
    icv_include_iv: bool,
) -> Result<(Vec<u8>, EspProtectedPacketSummary), DataplaneStateError> {
    let metadata = parse_esp_frame_metadata(frame)?;
    let plan = secrets.summary();
    let icv_len = esp_integrity_len(plan.integrity)?;
    let iv_bytes = esp_iv_bytes(plan.encryption)?;
    let min_len = ESP_HEADER_BYTES as usize + iv_bytes + icv_len;
    if frame.len() < min_len {
        return Err(DataplaneStateError::EspPacketTooShort);
    }
    let signed_len = frame.len() - icv_len;
    let (signed, received_icv) = frame.split_at(signed_len);
    let expected_input = if icv_include_iv {
        signed.to_vec()
    } else {
        // SPI | seq | ciphertext; skip the explicit IV between seq and
        // ciphertext when verifying against the non-standard convention.
        let iv_end = ESP_HEADER_BYTES as usize + iv_bytes;
        let mut input = Vec::with_capacity(8 + (signed_len - iv_end));
        input.extend_from_slice(&signed[..ESP_HEADER_BYTES as usize]);
        input.extend_from_slice(&signed[iv_end..]);
        input
    };
    let expected_icv = esp_integrity_tag(
        plan.integrity,
        secrets.inbound_integrity.expose_for_protocol(),
        &expected_input,
    )?;
    if !constant_time_eq(&expected_icv, received_icv) {
        return Err(DataplaneStateError::EspIntegrityMismatch);
    }
    let iv_start = ESP_HEADER_BYTES as usize;
    let iv_end = iv_start + iv_bytes;
    let iv = &frame[iv_start..iv_end];
    let ciphertext = &frame[iv_end..signed_len];
    let plaintext = if plan.encryption == "null" {
        ciphertext.to_vec()
    } else {
        esp_decrypt_cbc(
            plan.encryption,
            secrets.inbound_encryption.expose_for_protocol(),
            iv,
            ciphertext,
        )?
    };
    let (inner_packet, next_header) = strip_esp_padding(&plaintext)?;
    let inner_packet = inner_packet.to_vec();

    Ok((
        inner_packet.clone(),
        EspProtectedPacketSummary {
            sequence_number: metadata.sequence_number,
            outer_frame_bytes: frame.len(),
            protected_bytes: frame.len() - ESP_HEADER_BYTES as usize,
            inner_packet_bytes: inner_packet.len(),
            next_header,
            icv_bytes: icv_len,
            sensitive_values_policy: "packet_lengths_only_no_payload_or_key_material",
        },
    ))
}

pub fn wrap_ike_for_nat_t(ike_message: &[u8]) -> NattPacketSummary {
    NattPacketSummary {
        kind: NattPacketKind::IkeNonEspMarker.as_str(),
        udp_port: 4500,
        wire_bytes: 4 + ike_message.len(),
        esp: None,
        sensitive_values_policy: "ike_packet_body_not_serialized",
    }
}

fn wrap_esp_metadata_for_nat_t(esp: EspPacketMetadata) -> NattPacketSummary {
    NattPacketSummary {
        kind: NattPacketKind::EspInUdp4500.as_str(),
        udp_port: 4500,
        wire_bytes: esp.outer_frame_bytes,
        esp: Some(esp),
        sensitive_values_policy: "esp_packet_metadata_only",
    }
}

fn parse_esp_proposal(proposal: &'static str) -> DataplaneEspProposalPlan {
    let tokens = proposal.split('-').collect::<Vec<_>>();
    DataplaneEspProposalPlan {
        proposal,
        encryption: first_token(&tokens, |token| token.starts_with("aes")).unwrap_or("unknown"),
        integrity: first_token(&tokens, |token| {
            token.starts_with("sha") || token.starts_with("hmac")
        })
        .unwrap_or("profile_default"),
        encapsulation: "nat_t_udp_4500",
    }
}

fn first_token(tokens: &[&'static str], predicate: impl Fn(&str) -> bool) -> Option<&'static str> {
    tokens.iter().copied().find(|token| predicate(token))
}

fn classify_ip_version(packet: &[u8]) -> &'static str {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => "ipv4",
        Some(6) => "ipv6",
        _ => "unknown",
    }
}

fn protected_len_for_inner_packet(inner_packet_bytes: usize) -> usize {
    AES_CBC_IV_BYTES as usize
        + inner_packet_bytes
        + ESP_TRAILER_BUDGET_BYTES as usize
        + DEFAULT_INTEGRITY_CHECK_BYTES as usize
}

fn random_esp_iv() -> Result<[u8; 16], DataplaneStateError> {
    let mut iv = [0u8; 16];
    SystemRandom::new()
        .fill(&mut iv)
        .map_err(|_| DataplaneStateError::EspRandomFailed)?;
    Ok(iv)
}

fn esp_pad_len(payload_len: usize, block_bytes: usize) -> usize {
    let base = payload_len + 2;
    (block_bytes - (base % block_bytes)) % block_bytes
}

fn esp_integrity_len(integrity: &str) -> Result<usize, DataplaneStateError> {
    match integrity {
        "hmac_md5_96" => Ok(12),
        "hmac_sha1_96" => Ok(12),
        "hmac_sha256_128" => Ok(16),
        "hmac_sha384_192" => Ok(24),
        "hmac_sha512_256" => Ok(32),
        _ => Err(DataplaneStateError::EspUnsupportedIntegrity),
    }
}

fn esp_integrity_tag(
    integrity: &str,
    key: &[u8],
    message_without_icv: &[u8],
) -> Result<Vec<u8>, DataplaneStateError> {
    let (algorithm, icv_len) = match integrity {
        "hmac_md5_96" => (None, 12),
        "hmac_sha1_96" => (Some(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY), 12),
        "hmac_sha256_128" => (Some(hmac::HMAC_SHA256), 16),
        "hmac_sha384_192" => (Some(hmac::HMAC_SHA384), 24),
        "hmac_sha512_256" => (Some(hmac::HMAC_SHA512), 32),
        _ => return Err(DataplaneStateError::EspUnsupportedIntegrity),
    };
    let mut tag = match algorithm {
        Some(algorithm) => {
            let key = hmac::Key::new(algorithm, key);
            hmac::sign(&key, message_without_icv).as_ref().to_vec()
        }
        None => hmac_md5(key, message_without_icv),
    };
    tag.truncate(icv_len);
    Ok(tag)
}

fn esp_iv_bytes(encryption: &str) -> Result<usize, DataplaneStateError> {
    match encryption {
        "aes_cbc" => Ok(usize::try_from(AES_CBC_IV_BYTES).unwrap_or(16)),
        "null" => Ok(0),
        _ => Err(DataplaneStateError::EspUnsupportedCipher),
    }
}

fn hmac_md5(key: &[u8], data: &[u8]) -> Vec<u8> {
    const BLOCK_SIZE: usize = 64;
    let mut key = key.to_vec();
    if key.len() > BLOCK_SIZE {
        key = (*md5::compute(&key)).to_vec();
    }
    key.resize(BLOCK_SIZE, 0);

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }

    let mut inner_input = ipad.to_vec();
    inner_input.extend_from_slice(data);
    let inner = md5::compute(&inner_input);

    let mut outer_input = opad.to_vec();
    outer_input.extend_from_slice(inner.as_ref());
    (*md5::compute(&outer_input)).to_vec()
}

fn esp_encrypt_cbc(
    encryption: &str,
    key: &[u8],
    iv: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, DataplaneStateError> {
    match (encryption, key.len()) {
        ("null", 0) => Ok(plaintext.to_vec()),
        ("aes_cbc", 16) => Ok(cbc::Encryptor::<Aes128>::new_from_slices(key, iv)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher)?
            .encrypt_padded_vec_mut::<NoPadding>(plaintext)),
        ("aes_cbc", 32) => Ok(cbc::Encryptor::<Aes256>::new_from_slices(key, iv)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher)?
            .encrypt_padded_vec_mut::<NoPadding>(plaintext)),
        _ => Err(DataplaneStateError::EspUnsupportedCipher),
    }
}

fn esp_decrypt_cbc(
    encryption: &str,
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, DataplaneStateError> {
    match (encryption, key.len()) {
        ("null", 0) => Ok(ciphertext.to_vec()),
        ("aes_cbc", 16) => cbc::Decryptor::<Aes128>::new_from_slices(key, iv)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher)?
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher),
        ("aes_cbc", 32) => cbc::Decryptor::<Aes256>::new_from_slices(key, iv)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher)?
            .decrypt_padded_vec_mut::<NoPadding>(ciphertext)
            .map_err(|_| DataplaneStateError::EspUnsupportedCipher),
        _ => Err(DataplaneStateError::EspUnsupportedCipher),
    }
}

fn strip_esp_padding(plaintext: &[u8]) -> Result<(&[u8], u8), DataplaneStateError> {
    if plaintext.len() < 2 {
        return Err(DataplaneStateError::EspInvalidPadding);
    }
    let next_header = plaintext[plaintext.len() - 1];
    let pad_len = usize::from(plaintext[plaintext.len() - 2]);
    let payload_end = plaintext
        .len()
        .checked_sub(2 + pad_len)
        .ok_or(DataplaneStateError::EspInvalidPadding)?;
    let padding = &plaintext[payload_end..payload_end + pad_len];
    if !padding
        .iter()
        .copied()
        .enumerate()
        .all(|(index, value)| value == (index + 1) as u8)
    {
        return Err(DataplaneStateError::EspInvalidPadding);
    }
    Ok((&plaintext[..payload_end], next_header))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in left.iter().zip(right) {
        diff |= left ^ right;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::{
        ike_keys::{
            derive_child_sa_secret_pair, derive_ike_secret_bundle, ChildSaKeySchedulePlan,
            ChildSaSecretPair,
        },
        ike_payloads::{child_sa_proposal_from_profile_string, ike_proposal_from_profile_string},
        profiles::{GB_EE_23433, NL_VODAFONE_20404},
    };

    #[test]
    fn esp_md5_integrity_tag_matches_hmac_md5_vector() {
        assert_eq!(esp_integrity_len("hmac_md5_96").expect("len"), 12);
        let tag = esp_integrity_tag("hmac_md5_96", &[0x0bu8; 16], b"Hi There").expect("md5 tag");
        assert_eq!(
            tag,
            [0x92, 0x94, 0x72, 0x7a, 0x36, 0x38, 0xbb, 0x1c, 0x13, 0xf4, 0x8e, 0xf8,]
        );
    }

    fn ipv6_packet(len: usize) -> Vec<u8> {
        let mut packet = vec![0u8; len];
        packet[0] = 0x60;
        packet
    }

    fn test_child_sa_secrets() -> ChildSaSecretPair {
        let ike_proposal =
            ike_proposal_from_profile_string("aes128-sha256-modp2048", 1).expect("ike proposal");
        let ike_bundle = derive_ike_secret_bundle(
            &ike_proposal,
            &[0x11; 32],
            &[0x22; 32],
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            &[0x33; 256],
        )
        .expect("derive ike bundle");
        let esp_proposal =
            child_sa_proposal_from_profile_string("aes128-sha256", 1, &[0xaa, 0xbb, 0xcc, 0xdd])
                .expect("esp proposal");
        derive_child_sa_secret_pair(&ike_bundle, &esp_proposal, &[0x44; 32], &[0x55; 32])
            .expect("derive child sa")
    }

    fn reverse_secret_pair_for_test(secrets: &ChildSaSecretPair) -> ChildSaSecretPair {
        ChildSaSecretPair::from_test_parts(
            secrets.summary(),
            secrets.inbound_encryption.expose_for_test().to_vec(),
            secrets.inbound_integrity.expose_for_test().to_vec(),
            secrets.outbound_encryption.expose_for_test().to_vec(),
            secrets.outbound_integrity.expose_for_test().to_vec(),
        )
    }

    #[test]
    fn builds_userspace_esp_and_smoltcp_plan_for_gb_ee() {
        let plan = build_dataplane_plan(&GB_EE_23433);

        assert_eq!(plan.profile_id, "gb_ee_23433");
        assert_eq!(plan.outer_encapsulation, "esp_in_udp_nat_t");
        assert_eq!(plan.nat_t_port, 4500);
        assert_eq!(plan.nat_keepalive_seconds, 20);
        assert_eq!(plan.anti_replay_window, 64);
        assert_eq!(plan.mtu.inner_mtu, 1415);
        assert_eq!(plan.smoltcp.stack, "smoltcp");
        assert_eq!(plan.smoltcp.ip_stack, "ipv6");
        assert!(plan.smoltcp.tcp_enabled);
        assert_eq!(plan.esp_proposals[0].proposal, "aes128-sha256");
    }

    #[test]
    fn preserves_two_digit_mnc_profile_and_multiple_esp_candidates() {
        let plan = build_dataplane_plan(&NL_VODAFONE_20404);

        assert_eq!(plan.profile_id, "nl_vodafone_20404");
        assert_eq!(plan.plmn, "20404");
        assert_eq!(plan.esp_proposals[0].encryption, "aes256");
        assert_eq!(plan.esp_proposals[0].integrity, "sha256");
    }

    #[test]
    fn serialized_plan_has_no_sa_secrets_or_private_identifiers() {
        let plan = build_dataplane_plan(&GB_EE_23433);
        let json = serde_json::to_string(&plan).expect("serialize dataplane plan");

        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "spi",
            "ck",
            "ik",
            "sk_ai",
            "sk_ar",
            "sk_ei",
            "sk_er",
            "packet_payload",
            "inner_address",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "plan must not contain a {forbidden_key} field"
            );
        }
    }

    #[test]
    fn child_sa_state_reaches_inner_stack_without_serializing_sa_values() {
        let mut state = ChildSaStateMachine::new(&GB_EE_23433);

        state
            .negotiate_child_sa(0x1111_2222, 0x3333_4444)
            .expect("negotiate child sa");
        state
            .mark_esp_secrets_ready()
            .expect("mark ESP material ready");
        state.mark_inner_stack_ready().expect("mark smoltcp ready");

        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, "inner_stack_ready");
        assert_eq!(snapshot.selected_esp_proposal, Some("aes128-sha256"));
        assert!(snapshot.inbound_sa_identifier_present);
        assert!(snapshot.outbound_sa_identifier_present);
        assert_eq!(snapshot.smoltcp.stack, "smoltcp");
        assert_eq!(snapshot.inner_gateway.adapter, "smoltcp_virtual_device");

        let json = serde_json::to_string(&snapshot).expect("serialize child sa state");
        for forbidden in [
            "11112222",
            "33334444",
            "\"spi\"",
            "\"key_material\"",
            "\"plaintext\"",
            "\"ciphertext\"",
            "\"frame_body\"",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn anti_replay_accepts_new_frames_and_rejects_duplicate_or_old_frames() {
        let mut window = AntiReplayWindow::new(4);

        assert_eq!(window.accept(8).decision, "accepted_new_highest");
        assert_eq!(window.accept(7).decision, "accepted_within_window");
        assert_eq!(window.accept(7).decision, "rejected_duplicate");
        assert_eq!(window.accept(3).decision, "rejected_too_old");
        assert_eq!(window.accept(0).decision, "rejected_invalid_sequence");

        let snapshot = window.snapshot();
        assert_eq!(snapshot.window_size, 4);
        assert_eq!(snapshot.highest_sequence, 8);
        assert_eq!(snapshot.tracked_slots, 2);
    }

    #[test]
    fn esp_packet_metadata_parse_and_build_redacts_identifier_and_body() {
        let frame = build_esp_frame_bytes(0x0102_0304, 7, &[0xaa; 24]).expect("build frame");
        let metadata = parse_esp_frame_metadata(&frame).expect("parse frame");

        assert!(metadata.sa_identifier_present);
        assert_eq!(metadata.sequence_number, 7);
        assert_eq!(metadata.protected_bytes, 24);
        assert_eq!(metadata.outer_frame_bytes, 32);

        let json = serde_json::to_string(&metadata).expect("serialize metadata");
        for forbidden in [
            "01020304",
            "aaaaaaaa",
            "\"spi\"",
            "\"plaintext\"",
            "\"ciphertext\"",
            "\"packet_payload\"",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn nat_t_parser_classifies_non_esp_marker_keepalive_and_esp() {
        let ike = parse_nat_t_packet(&[0, 0, 0, 0, 0x21, 0x20]).expect("parse ike marker");
        assert_eq!(ike.kind, "ike_non_esp_marker");
        assert!(ike.esp.is_none());

        let keepalive = parse_nat_t_packet(&[0xff]).expect("parse keepalive");
        assert_eq!(keepalive.kind, "nat_keepalive");

        let frame = build_esp_frame_bytes(0x0102_0304, 9, &[0xbb; 16]).expect("build esp");
        let esp = parse_nat_t_packet(&frame).expect("parse esp");
        assert_eq!(esp.kind, "esp_in_udp_4500");
        assert_eq!(esp.esp.as_ref().map(|item| item.sequence_number), Some(9));
    }

    #[test]
    fn esp_protects_and_unprotects_inner_packet_without_serializing_payload_or_keys() {
        let secrets = test_child_sa_secrets();
        let inbound_secrets = reverse_secret_pair_for_test(&secrets);
        let inner = ipv6_packet(96);

        let (frame, protected_summary) =
            protect_inner_packet_for_esp(0x0102_0304, 7, &inner, 59, &secrets)
                .expect("protect inner packet");
        let (decoded, decoded_summary) =
            unprotect_inner_packet_from_esp(&frame, &inbound_secrets).expect("unprotect packet");

        assert_eq!(decoded, inner);
        assert_eq!(protected_summary.sequence_number, 7);
        assert_eq!(protected_summary.inner_packet_bytes, 96);
        assert_eq!(protected_summary.next_header, 59);
        assert_eq!(protected_summary.icv_bytes, 16);
        assert_eq!(decoded_summary.sequence_number, 7);
        assert_eq!(decoded_summary.inner_packet_bytes, 96);
        assert_eq!(decoded_summary.next_header, 59);
        assert_eq!(parse_esp_frame_metadata(&frame).unwrap().sequence_number, 7);

        let json = serde_json::to_string(&decoded_summary).expect("serialize protected summary");
        for forbidden in [
            "01020304",
            "60000000",
            "\"spi\"",
            "\"key\"",
            "\"plaintext\"",
            "\"ciphertext\"",
            "\"packet_payload\"",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }

        let mut tampered = frame;
        let last = tampered.last_mut().expect("frame has ICV");
        *last ^= 0x01;
        let err =
            unprotect_inner_packet_from_esp(&tampered, &inbound_secrets).expect_err("tamper fails");
        assert_eq!(err, DataplaneStateError::EspIntegrityMismatch);
    }

    #[test]
    fn esp_icv_excludes_iv_mode_round_trips_and_is_distinct_from_standard() {
        let secrets = test_child_sa_secrets();
        let inbound_secrets = reverse_secret_pair_for_test(&secrets);
        let inner = ipv6_packet(96);

        let (frame, _) =
            protect_inner_packet_for_esp_with_mode(0x0102_0304, 7, &inner, 59, &secrets, false)
                .expect("protect with ICV excluding IV");
        let (decoded, _) =
            unprotect_inner_packet_from_esp_with_mode(&frame, &inbound_secrets, false)
                .expect("unprotect with ICV excluding IV");
        assert_eq!(decoded, inner);

        // The alternate mode must not verify under the standard convention.
        let err = unprotect_inner_packet_from_esp(&frame, &inbound_secrets)
            .expect_err("standard ICV check rejects alternate mode frame");
        assert_eq!(err, DataplaneStateError::EspIntegrityMismatch);
    }

    #[test]
    fn esp_null_encryption_round_trips_without_iv_or_cipher_key() {
        let plan = ChildSaKeySchedulePlan {
            encryption: "null",
            integrity: "hmac_sha1_96",
            encryption_key_bytes: 0,
            integrity_key_bytes: 20,
            direction_secret_bytes: 20,
            total_secret_bytes: 40,
            exported_secret_values: false,
            sensitive_values_policy: "test_null_encryption",
        };
        let secrets = ChildSaSecretPair::from_test_parts(
            plan,
            Vec::new(),
            vec![0x44; 20],
            Vec::new(),
            vec![0x55; 20],
        );
        let inbound = ChildSaSecretPair::from_test_parts(
            secrets.summary(),
            Vec::new(),
            vec![0x55; 20],
            Vec::new(),
            vec![0x44; 20],
        );
        let inner = ipv6_packet(64);

        let (frame, protected) = protect_inner_packet_for_esp(0x0102_0304, 3, &inner, 59, &secrets)
            .expect("protect null-encrypted packet");
        let (decoded, _) = unprotect_inner_packet_from_esp(&frame, &inbound).expect("unprotect");

        assert_eq!(decoded, inner);
        assert_eq!(protected.inner_packet_bytes, 64);
        assert_eq!(protected.icv_bytes, 12);
        assert_eq!(esp_iv_bytes("null").expect("null iv bytes"), 0);
        assert_eq!(esp_iv_bytes("aes_cbc").expect("aes iv bytes"), 16);
    }

    #[test]
    fn dataplane_state_records_frame_metadata_only() {
        let mut state = ChildSaStateMachine::new(&NL_VODAFONE_20404);
        state
            .negotiate_child_sa(0x0102_0304, 0x0506_0708)
            .expect("negotiate child sa");
        state
            .mark_esp_secrets_ready()
            .expect("mark ESP material ready");
        state.mark_inner_stack_ready().expect("mark smoltcp ready");

        let inbound = state
            .record_inbound_esp_frame(1, 152)
            .expect("record inbound frame");
        let outbound = state
            .record_outbound_esp_frame(184)
            .expect("record outbound frame");

        assert!(inbound.accepted);
        assert_eq!(outbound.direction, "outbound");
        assert_eq!(outbound.sequence_number, Some(1));
        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, "forwarding");
        assert_eq!(snapshot.packets_in, 1);
        assert_eq!(snapshot.packets_out, 1);
        assert_eq!(snapshot.bytes_in, 152);
        assert_eq!(snapshot.bytes_out, 184);

        let json = serde_json::to_string(&snapshot).expect("serialize dataplane state");
        for forbidden_key in [
            "\"packet_payload\"",
            "\"inner_address\"",
            "\"plaintext\"",
            "\"ciphertext\"",
            "\"key_material\"",
            "01020304",
            "05060708",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden_key));
        }
    }

    #[test]
    fn outbound_inner_packets_allocate_sequences_and_update_inner_gateway() {
        let mut state = ChildSaStateMachine::new(&GB_EE_23433);
        state
            .negotiate_child_sa(0x1010_2020, 0x3030_4040)
            .expect("negotiate child sa");
        state.mark_esp_secrets_ready().expect("esp ready");
        state.mark_inner_stack_ready().expect("inner ready");

        let first = state
            .record_outbound_inner_packet(&ipv6_packet(240))
            .expect("send first inner packet");
        let second = state
            .record_outbound_inner_packet(&ipv6_packet(128))
            .expect("send second inner packet");

        assert!(first.accepted);
        assert_eq!(first.sequence_number, Some(1));
        assert_eq!(second.sequence_number, Some(2));
        assert_eq!(
            second.natt.as_ref().map(|summary| summary.kind),
            Some("esp_in_udp_4500")
        );

        let snapshot = state.snapshot();
        assert_eq!(snapshot.sa_pair.outbound_sequence_allocated, 2);
        assert_eq!(snapshot.inner_gateway.queued_packets, 2);
        assert_eq!(snapshot.inner_gateway.packets_to_esp, 2);
        assert_eq!(snapshot.packets_out, 2);
    }

    #[test]
    fn mtu_drop_is_metadata_only_and_does_not_allocate_sequence() {
        let mut state = ChildSaStateMachine::new(&GB_EE_23433);
        state
            .negotiate_child_sa(0x1010_2020, 0x3030_4040)
            .expect("negotiate child sa");
        state.mark_esp_secrets_ready().expect("esp ready");
        state.mark_inner_stack_ready().expect("inner ready");

        let decision = state
            .record_outbound_inner_packet(&ipv6_packet(1450))
            .expect("oversize is reported as drop metadata");

        assert!(!decision.accepted);
        assert_eq!(decision.decision, "mtu_drop");
        assert_eq!(decision.sequence_number, None);
        assert_eq!(
            decision.mtu.as_ref().map(|mtu| mtu.decision),
            Some("mtu_exceeded")
        );

        let snapshot = state.snapshot();
        assert_eq!(snapshot.mtu_drops, 1);
        assert_eq!(snapshot.sa_pair.outbound_sequence_allocated, 0);
        assert_eq!(snapshot.inner_gateway.queued_packets, 0);
        assert_eq!(
            snapshot
                .inner_gateway
                .last_packet
                .as_ref()
                .and_then(|packet| packet.drop_reason),
            Some("mtu_exceeded")
        );

        let json = serde_json::to_string(&snapshot).expect("serialize mtu drop");
        for forbidden in ["plaintext", "ciphertext", "packet_payload", "30304040"] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn inbound_nat_t_esp_updates_replay_and_inner_gateway() {
        let mut state = ChildSaStateMachine::new(&GB_EE_23433);
        state
            .negotiate_child_sa(0x0102_0304, 0x0506_0708)
            .expect("negotiate child sa");
        state.mark_esp_secrets_ready().expect("esp ready");
        state.mark_inner_stack_ready().expect("inner ready");
        let frame = build_esp_frame_bytes(0x0102_0304, 5, &[0xcc; 48]).expect("build frame");

        let accepted = state
            .record_inbound_nat_t_packet(&frame)
            .expect("record inbound nat-t");
        let duplicate = state
            .record_inbound_nat_t_packet(&frame)
            .expect("record duplicate nat-t");

        assert!(accepted.accepted);
        assert_eq!(accepted.sequence_number, Some(5));
        assert!(!duplicate.accepted);
        assert_eq!(duplicate.decision, "rejected_duplicate");

        let snapshot = state.snapshot();
        assert_eq!(snapshot.replay_window.highest_sequence, 5);
        assert_eq!(snapshot.packets_in, 1);
        assert_eq!(snapshot.inner_gateway.packets_from_esp, 1);
    }

    #[test]
    fn rekey_and_teardown_stop_frame_processing_without_reusing_sa_state() {
        let mut state = ChildSaStateMachine::new(&GB_EE_23433);
        state
            .negotiate_child_sa(0x0102_0304, 0x0506_0708)
            .expect("negotiate child sa");
        state.mark_esp_secrets_ready().expect("esp ready");
        state.mark_inner_stack_ready().expect("inner ready");
        state.begin_rekey().expect("begin rekey");

        let err = state
            .record_outbound_esp_frame(96)
            .expect_err("rekey rejects frame processing");
        assert!(matches!(err, DataplaneStateError::InvalidPhase { .. }));

        state.teardown();
        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, "teardown");
        assert!(!snapshot.inbound_sa_identifier_present);
        assert!(!snapshot.outbound_sa_identifier_present);
    }
}
