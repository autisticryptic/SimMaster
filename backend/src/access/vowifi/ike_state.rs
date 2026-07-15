#![allow(dead_code)]

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use serde::Serialize;

use super::{
    ike_codec::{IkeCodecError, IkeExchangeType, IkeMessage, IkePayload, IkePayloadType},
    ike_eap::{parse_eap_aka_summary, EapAkaError, EapAkaPacketSummary},
    ike_encrypted::{
        build_encrypted_metadata_payload, build_encrypted_payload, build_encrypted_payload_plan,
        decrypt_encrypted_payload_from_message, encode_encrypted_message,
        encrypted_message_from_payload, EncryptedPayloadError, EncryptedPayloadPlan,
        IkeSkDirection,
    },
    ike_events::{parse_control_event, IkeControlEvent, IkeControlEventError},
    ike_keys::{
        derive_child_sa_secret_pair, derive_ike_secret_bundle, ChildSaSecretPair, IkeKeyError,
        IkeKeySchedulePlan, IkeSecretBundle,
    },
    ike_payloads::{
        build_authentication_shared_key_payload, build_configuration_request_payload,
        build_identification_initiator_payload, build_identification_responder_payload,
        build_ke_payload, build_nonce_payload, build_notify_payload, build_sa_payload,
        build_traffic_selector_initiator_payload, build_traffic_selector_responder_payload,
        child_sa_proposal_from_profile_string, ike_proposal_dh_group_from_profile_string,
        ike_proposal_from_profile_string, parse_sa_payload, IkeProtocolId, NotifyProtocolId,
        ParsedProposal, PayloadBuildError, ProposalParseError, SaParseError,
        CFG_ATTR_INTERNAL_IP4_ADDRESS, CFG_ATTR_INTERNAL_IP4_DNS, CFG_ATTR_INTERNAL_IP4_PCSCF,
        CFG_ATTR_INTERNAL_IP6_ADDRESS, CFG_ATTR_INTERNAL_IP6_DNS, CFG_ATTR_INTERNAL_IP6_PCSCF,
        DH_MODP_2048, NOTIFY_EAP_ONLY_AUTHENTICATION, NOTIFY_NAT_DETECTION_DESTINATION_IP,
        NOTIFY_NAT_DETECTION_SOURCE_IP,
    },
    ike_retransmit::{RetransmitPolicy, RetransmitState},
    profiles::CarrierProfile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IkeStatePhase {
    Idle,
    SaInitRequestBuilt,
    SaInitResponseAccepted,
    SessionKeysReady,
    AuthRequestBuilt,
    EapAkaChallengeReceived,
    EapAkaIdentityReceived,
    UsimAkaResponsePending,
    EapAkaNotificationReceived,
    EapAkaResponseReady,
    AuthSuccessAccepted,
    ChildSaReady,
    RekeyPending,
    ReauthRequired,
    ChildSaMissing,
    TeardownRequested,
    Failed,
}

impl IkeStatePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::SaInitRequestBuilt => "sa_init_request_built",
            Self::SaInitResponseAccepted => "sa_init_response_accepted",
            Self::SessionKeysReady => "session_keys_ready",
            Self::AuthRequestBuilt => "auth_request_built",
            Self::EapAkaChallengeReceived => "eap_aka_challenge_received",
            Self::EapAkaIdentityReceived => "eap_aka_identity_received",
            Self::UsimAkaResponsePending => "usim_aka_response_pending",
            Self::EapAkaNotificationReceived => "eap_aka_notification_received",
            Self::EapAkaResponseReady => "eap_aka_response_ready",
            Self::AuthSuccessAccepted => "auth_success_accepted",
            Self::ChildSaReady => "child_sa_ready",
            Self::RekeyPending => "rekey_pending",
            Self::ReauthRequired => "reauth_required",
            Self::ChildSaMissing => "child_sa_missing",
            Self::TeardownRequested => "teardown_requested",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeTranscriptEvent {
    pub message_id: u32,
    pub exchange: &'static str,
    pub direction: &'static str,
    pub payloads: Vec<&'static str>,
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkePublicSnapshot {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub phase: &'static str,
    pub initiator_spi_present: bool,
    pub responder_spi_present: bool,
    pub next_message_id: u32,
    pub selected_proposal: Option<&'static str>,
    pub key_schedule: Option<IkeKeySchedulePlan>,
    pub encrypted_payload: Option<EncryptedPayloadPlan>,
    pub retransmit: RetransmitState,
    pub last_control_event: Option<IkeControlEvent>,
    pub eap: Option<EapAkaPacketSummary>,
    pub child_sa: Option<IkeChildSaSummary>,
    pub transcript: Vec<IkeTranscriptEvent>,
    pub last_error: Option<String>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IkeChildSaSummary {
    pub responder_auth_present: bool,
    pub child_sa_present: bool,
    pub selected_esp_proposal_present: bool,
    pub inbound_sa_identifier_present: bool,
    pub outbound_sa_identifier_present: bool,
    pub configuration_reply_present: bool,
    pub assigned_inner_address_present: bool,
    pub assigned_inner_address_count: usize,
    pub assigned_ipv6_prefix_length_present: bool,
    pub pcscf_present: bool,
    pub pcscf_count: usize,
    pub dns_present: bool,
    pub dns_count: usize,
    pub traffic_selectors_present: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkeChildSaMaterial {
    pub inbound_sa_identifier: u32,
    pub outbound_sa_identifier: u32,
    pub selected_profile_proposal: &'static str,
    pub proposal: ParsedProposal,
    pub configuration: Option<IkeConfigurationMaterial>,
    pub secrets: ChildSaSecretPair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkeConfigurationMaterial {
    pub assigned_inner_addresses: Vec<IpAddr>,
    pub assigned_ipv6_prefix_length: Option<u8>,
    pub pcscf_addresses: Vec<IpAddr>,
    pub dns_addresses: Vec<IpAddr>,
}

#[derive(Clone, PartialEq, Eq)]
// The EAP prefix is intentional: it distinguishes protocol-round outcomes from
// the surrounding IKE_AUTH state machine when values appear in traces.
#[allow(clippy::enum_variant_names)]
pub enum IkeAuthProgress {
    EapAkaIdentity { packet: Vec<u8> },
    EapAkaNotification { packet: Vec<u8> },
    EapSuccess { child_sa_included: bool },
}

impl fmt::Debug for IkeAuthProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EapAkaIdentity { packet } => f
                .debug_struct("EapAkaIdentity")
                .field("packet_len", &packet.len())
                .field("packet_redacted", &true)
                .finish(),
            Self::EapAkaNotification { packet } => f
                .debug_struct("EapAkaNotification")
                .field("packet_len", &packet.len())
                .field("packet_redacted", &true)
                .finish(),
            Self::EapSuccess { child_sa_included } => f
                .debug_struct("EapSuccess")
                .field("child_sa_included", child_sa_included)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkePrivateContext {
    pub initiator_spi: u64,
    pub responder_spi: Option<u64>,
    pub initiator_nonce: Vec<u8>,
    pub responder_nonce: Option<Vec<u8>>,
    pub initiator_public_dh: Vec<u8>,
    pub initiator_dh_group: u16,
    pub responder_public_dh: Option<Vec<u8>>,
    pub selected_proposal: Option<&'static str>,
    pub offered_ike_proposals: Vec<&'static str>,
    pub secret_bundle: Option<IkeSecretBundle>,
    pub encrypted_payload: Option<EncryptedPayloadPlan>,
    pub sa_init_request_packet: Option<Vec<u8>>,
    pub sa_init_response_packet: Option<Vec<u8>>,
    pub initiator_id_payload_body: Option<Vec<u8>>,
    pub child_sa_initiator_spi: Option<[u8; 4]>,
    pub child_sa_material: Option<IkeChildSaMaterial>,
    pub nat_t_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IkeStateMachine {
    profile: &'static CarrierProfile,
    phase: IkeStatePhase,
    next_message_id: u32,
    retransmit_policy: RetransmitPolicy,
    retransmit: RetransmitState,
    private: IkePrivateContext,
    eap: Option<EapAkaPacketSummary>,
    child_sa: Option<IkeChildSaSummary>,
    transcript: Vec<IkeTranscriptEvent>,
    last_control_event: Option<IkeControlEvent>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IkeStateError {
    EmptyProfileProposals,
    PayloadBuild(PayloadBuildError),
    ProposalParse(ProposalParseError),
    SaParse(SaParseError),
    KeySchedule(IkeKeyError),
    EncryptedPayload(EncryptedPayloadError),
    Codec(IkeCodecError),
    ControlEvent(IkeControlEventError),
    Eap(EapAkaError),
    InvalidPhase {
        expected: &'static str,
        actual: &'static str,
    },
    InvalidResponse {
        reason: &'static str,
    },
}

impl fmt::Display for IkeStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProfileProposals => write!(f, "profile has no IKE proposals"),
            Self::PayloadBuild(err) => write!(f, "{err}"),
            Self::ProposalParse(err) => write!(f, "{err}"),
            Self::SaParse(err) => write!(f, "{err}"),
            Self::KeySchedule(err) => write!(f, "{err}"),
            Self::EncryptedPayload(err) => write!(f, "{err}"),
            Self::Codec(err) => write!(f, "{err}"),
            Self::ControlEvent(err) => write!(f, "{err}"),
            Self::Eap(err) => write!(f, "{err}"),
            Self::InvalidPhase { expected, actual } => {
                write!(f, "invalid IKE phase expected={expected} actual={actual}")
            }
            Self::InvalidResponse { reason } => write!(f, "invalid IKE response: {reason}"),
        }
    }
}

impl std::error::Error for IkeStateError {}

impl From<PayloadBuildError> for IkeStateError {
    fn from(value: PayloadBuildError) -> Self {
        Self::PayloadBuild(value)
    }
}

impl From<ProposalParseError> for IkeStateError {
    fn from(value: ProposalParseError) -> Self {
        Self::ProposalParse(value)
    }
}

impl From<SaParseError> for IkeStateError {
    fn from(value: SaParseError) -> Self {
        Self::SaParse(value)
    }
}

impl From<IkeKeyError> for IkeStateError {
    fn from(value: IkeKeyError) -> Self {
        Self::KeySchedule(value)
    }
}

impl From<EncryptedPayloadError> for IkeStateError {
    fn from(value: EncryptedPayloadError) -> Self {
        Self::EncryptedPayload(value)
    }
}

impl From<IkeControlEventError> for IkeStateError {
    fn from(value: IkeControlEventError) -> Self {
        Self::ControlEvent(value)
    }
}
impl From<IkeCodecError> for IkeStateError {
    fn from(value: IkeCodecError) -> Self {
        Self::Codec(value)
    }
}

impl From<EapAkaError> for IkeStateError {
    fn from(value: EapAkaError) -> Self {
        Self::Eap(value)
    }
}

impl IkeStateMachine {
    pub fn new(
        profile: &'static CarrierProfile,
        initiator_spi: u64,
        initiator_nonce: Vec<u8>,
        initiator_public_dh: Vec<u8>,
    ) -> Self {
        Self::new_with_dh_group(
            profile,
            initiator_spi,
            initiator_nonce,
            initiator_public_dh,
            DH_MODP_2048,
        )
    }

    pub fn new_with_dh_group(
        profile: &'static CarrierProfile,
        initiator_spi: u64,
        initiator_nonce: Vec<u8>,
        initiator_public_dh: Vec<u8>,
        initiator_dh_group: u16,
    ) -> Self {
        let retransmit_policy = RetransmitPolicy::default();
        Self {
            profile,
            phase: IkeStatePhase::Idle,
            next_message_id: 0,
            retransmit: retransmit_policy.initial_state(0),
            retransmit_policy,
            private: IkePrivateContext {
                initiator_spi,
                responder_spi: None,
                initiator_nonce,
                responder_nonce: None,
                initiator_public_dh,
                initiator_dh_group,
                responder_public_dh: None,
                selected_proposal: None,
                offered_ike_proposals: Vec::new(),
                secret_bundle: None,
                encrypted_payload: None,
                sa_init_request_packet: None,
                sa_init_response_packet: None,
                initiator_id_payload_body: None,
                child_sa_initiator_spi: None,
                child_sa_material: None,
                nat_t_supported: false,
            },
            eap: None,
            child_sa: None,
            transcript: Vec::new(),
            last_control_event: None,
            last_error: None,
        }
    }

    pub fn snapshot(&self) -> IkePublicSnapshot {
        IkePublicSnapshot {
            profile_id: self.profile.meta.profile_id,
            plmn: self.profile.meta.plmn,
            phase: self.phase.as_str(),
            initiator_spi_present: self.private.initiator_spi != 0,
            responder_spi_present: self.private.responder_spi.is_some(),
            next_message_id: self.next_message_id,
            selected_proposal: self.private.selected_proposal,
            key_schedule: self
                .private
                .secret_bundle
                .as_ref()
                .map(|bundle| bundle.summary()),
            encrypted_payload: self.private.encrypted_payload.clone(),
            retransmit: self.retransmit.clone(),
            eap: self.eap.clone(),
            child_sa: self.child_sa.clone(),
            transcript: self.transcript.clone(),
            last_control_event: self.last_control_event.clone(),
            last_error: self.last_error.clone(),
            sensitive_values_policy: "handshake_material_never_serialized",
        }
    }

    pub fn responder_public_dh(&self) -> Option<&[u8]> {
        self.private.responder_public_dh.as_deref()
    }

    pub fn build_sa_init_request(&mut self) -> Result<IkeMessage, IkeStateError> {
        self.build_sa_init_request_for_transport(None, None)
    }

    pub fn build_sa_init_request_for_addresses(
        &mut self,
        source: SocketAddr,
        destination: SocketAddr,
    ) -> Result<IkeMessage, IkeStateError> {
        self.build_sa_init_request_for_transport(Some((source, destination)), None)
    }

    pub fn build_sa_init_request_for_addresses_with_proposals(
        &mut self,
        source: SocketAddr,
        destination: SocketAddr,
        proposal_texts: &[&'static str],
    ) -> Result<IkeMessage, IkeStateError> {
        self.build_sa_init_request_for_transport(Some((source, destination)), Some(proposal_texts))
    }

    fn build_sa_init_request_for_transport(
        &mut self,
        transport_addrs: Option<(SocketAddr, SocketAddr)>,
        proposal_texts: Option<&[&'static str]>,
    ) -> Result<IkeMessage, IkeStateError> {
        self.require_phase(IkeStatePhase::Idle)?;

        let requested_proposals = proposal_texts.unwrap_or(self.profile.ikev2.ike_proposals);
        if requested_proposals.is_empty() {
            return Err(IkeStateError::EmptyProfileProposals);
        }
        let mut effective_proposals = Vec::new();
        for proposal_text in requested_proposals {
            if ike_proposal_dh_group_from_profile_string(proposal_text)?
                != self.private.initiator_dh_group
            {
                if proposal_texts.is_some() {
                    return Err(IkeStateError::InvalidResponse {
                        reason: "proposal_dh_group_mismatch",
                    });
                }
                continue;
            }
            effective_proposals.push(*proposal_text);
        }
        if effective_proposals.is_empty() {
            return Err(IkeStateError::InvalidResponse {
                reason: "no_proposal_for_initiator_dh_group",
            });
        }
        let proposals = effective_proposals
            .iter()
            .enumerate()
            .map(|(index, proposal_text)| {
                ike_proposal_from_profile_string(proposal_text, (index + 1) as u8)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut payloads = vec![
            build_sa_payload(&proposals)?,
            build_ke_payload(
                self.private.initiator_dh_group,
                &self.private.initiator_public_dh,
            ),
            build_nonce_payload(&self.private.initiator_nonce),
        ];
        if let Some((source, destination)) = transport_addrs {
            payloads.push(build_notify_payload(
                NotifyProtocolId::None,
                &[],
                NOTIFY_NAT_DETECTION_SOURCE_IP,
                &nat_detection_hash(self.private.initiator_spi, 0, source),
            )?);
            payloads.push(build_notify_payload(
                NotifyProtocolId::None,
                &[],
                NOTIFY_NAT_DETECTION_DESTINATION_IP,
                &nat_detection_hash(self.private.initiator_spi, 0, destination),
            )?);
        }
        let message = IkeMessage::new_request(
            self.private.initiator_spi,
            IkeExchangeType::IkeSaInit,
            0,
            payloads,
        );

        self.private.offered_ike_proposals = effective_proposals.clone();
        self.private.selected_proposal = effective_proposals.first().copied();
        self.private.sa_init_request_packet = Some(message.encode()?);
        self.phase = IkeStatePhase::SaInitRequestBuilt;
        self.retransmit = self.retransmit_policy.initial_state(0);
        self.push_event(&message, "outbound", "sa_init_request_built");
        Ok(message)
    }

    pub fn accept_sa_init_response(&mut self, response: &[u8]) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::SaInitRequestBuilt)?;
        let message = IkeMessage::decode(response)?;
        if message.header.exchange_type != IkeExchangeType::IkeSaInit {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "unexpected_exchange",
            });
        }
        if !message.header.flags.response {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "expected_response_flag",
            });
        }
        if message.header.initiator_spi != self.private.initiator_spi {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "initiator_spi_mismatch",
            });
        }
        if message.header.responder_spi == 0 {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            });
        }
        let Some(sa_payload) = find_payload(&message, IkePayloadType::SecurityAssociation) else {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "missing_sa_init_payload",
            });
        };
        let Some(ke_payload) = find_payload(&message, IkePayloadType::KeyExchange) else {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "missing_sa_init_payload",
            });
        };
        let Some(nonce_payload) = find_payload(&message, IkePayloadType::Nonce) else {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "missing_sa_init_payload",
            });
        };

        let parsed_proposals = match parse_sa_payload(&sa_payload.body) {
            Ok(proposals) => proposals,
            Err(err) => return self.fail(IkeStateError::SaParse(err)),
        };
        let Some(selected_proposal) =
            matching_ike_profile_proposal(&parsed_proposals, &self.private.offered_ike_proposals)
        else {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "selected_sa_not_offered_by_profile",
            });
        };

        self.private.selected_proposal = Some(selected_proposal);
        self.private.responder_spi = Some(message.header.responder_spi);
        self.private.nat_t_supported = message.payloads.iter().any(|payload| {
            payload.payload_type == IkePayloadType::Notify
                && notify_type(&payload.body).is_some_and(|notify| {
                    notify == NOTIFY_NAT_DETECTION_SOURCE_IP
                        || notify == NOTIFY_NAT_DETECTION_DESTINATION_IP
                })
        });
        if ke_payload.body.len() < 4 {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "invalid_ke_payload",
            });
        }
        self.private.responder_public_dh = Some(ke_payload.body[4..].to_vec());
        self.private.responder_nonce = Some(nonce_payload.body.clone());
        self.private.sa_init_response_packet = Some(response.to_vec());
        self.phase = IkeStatePhase::SaInitResponseAccepted;
        self.next_message_id = 1;
        self.retransmit = self.retransmit_policy.initial_state(1);
        self.push_event(&message, "inbound", "sa_init_response_accepted");
        Ok(())
    }

    pub fn nat_t_supported(&self) -> bool {
        self.private.nat_t_supported
    }

    pub fn derive_session_keys(&mut self, shared_secret: &[u8]) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::SaInitResponseAccepted)?;
        let selected_proposal =
            self.private
                .selected_proposal
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_selected_proposal",
                })?;
        let responder_spi = self
            .private
            .responder_spi
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            })?;
        let responder_nonce =
            self.private
                .responder_nonce
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_responder_nonce",
                })?;
        let proposal = ike_proposal_from_profile_string(selected_proposal, 1)?;
        let bundle = derive_ike_secret_bundle(
            &proposal,
            &self.private.initiator_nonce,
            responder_nonce,
            self.private.initiator_spi,
            responder_spi,
            shared_secret,
        )?;

        self.private.secret_bundle = Some(bundle);
        self.phase = IkeStatePhase::SessionKeysReady;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_sa_init",
            direction: "local",
            payloads: vec!["key_schedule_summary"],
            note: "session_keys_ready",
        });
        Ok(())
    }

    pub fn build_auth_eap_start_request(&mut self) -> Result<IkeMessage, IkeStateError> {
        self.require_phase(IkeStatePhase::SessionKeysReady)?;
        let responder_spi = self
            .private
            .responder_spi
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            })?;
        let key_schedule =
            self.private
                .secret_bundle
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_session_keys",
                })?;
        let inner_payloads = vec![encrypted_inner_identity_placeholder()];
        let encrypted_plan =
            build_encrypted_payload_plan(&key_schedule.summary(), &inner_payloads)?;
        let encrypted_payload = build_encrypted_metadata_payload(&encrypted_plan);
        let message = IkeMessage::new_request(
            self.private.initiator_spi,
            IkeExchangeType::IkeAuth,
            self.next_message_id,
            vec![encrypted_payload],
        )
        .with_responder_spi(responder_spi);

        self.phase = IkeStatePhase::AuthRequestBuilt;
        self.private.encrypted_payload = Some(encrypted_plan);
        self.retransmit = self
            .retransmit_policy
            .initial_state(message.header.message_id);
        self.push_event(&message, "outbound", "auth_eap_start_request_built");
        Ok(message)
    }

    pub fn build_auth_eap_start_packet(&mut self) -> Result<Vec<u8>, IkeStateError> {
        self.build_auth_eap_start_packet_for_identity("simadmin-inner-identity-metadata")
    }

    pub fn build_auth_eap_start_packet_for_identity(
        &mut self,
        identity: &str,
    ) -> Result<Vec<u8>, IkeStateError> {
        self.require_phase(IkeStatePhase::SessionKeysReady)?;
        let responder_spi = self
            .private
            .responder_spi
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            })?;
        let key_schedule =
            self.private
                .secret_bundle
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_session_keys",
                })?;
        let child_sa_initiator_spi = self.child_sa_initiator_spi()?;
        let inner_payloads = self.build_auth_init_payloads(identity, child_sa_initiator_spi)?;
        let initiator_id_payload_body = inner_payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::IdentificationInitiator)
            .map(|payload| payload.body.clone());
        let encrypted_plan =
            build_encrypted_payload_plan(&key_schedule.summary(), &inner_payloads)?;
        let first_inner_payload = inner_payloads[0].payload_type;
        let encrypted_payload = build_encrypted_payload(
            key_schedule,
            IkeSkDirection::InitiatorToResponder,
            &inner_payloads,
        )?;
        let message = encrypted_message_from_payload(
            self.private.initiator_spi,
            responder_spi,
            IkeExchangeType::IkeAuth,
            true,
            self.next_message_id,
            encrypted_payload,
        );
        let packet = encode_encrypted_message(
            &message,
            first_inner_payload,
            key_schedule,
            IkeSkDirection::InitiatorToResponder,
        )?;

        self.phase = IkeStatePhase::AuthRequestBuilt;
        self.private.encrypted_payload = Some(encrypted_plan);
        self.private.initiator_id_payload_body = initiator_id_payload_body;
        self.private.child_sa_initiator_spi = Some(child_sa_initiator_spi);
        self.retransmit = self
            .retransmit_policy
            .initial_state(message.header.message_id);
        self.push_event(&message, "outbound", "auth_eap_start_packet_built");
        Ok(packet)
    }

    fn build_auth_init_payloads(
        &self,
        identity: &str,
        child_sa_initiator_spi: [u8; 4],
    ) -> Result<Vec<IkePayload>, IkeStateError> {
        let first_esp_proposal =
            self.profile
                .ikev2
                .esp_proposals
                .first()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "profile_missing_esp_proposal",
                })?;
        let mut payloads = vec![
            build_identification_initiator_payload(identity),
            build_configuration_request_payload(self.profile.epdg.ip_stack),
            build_sa_payload(&[child_sa_proposal_from_profile_string(
                first_esp_proposal,
                1,
                &child_sa_initiator_spi,
            )?])?,
            build_traffic_selector_initiator_payload(self.profile.epdg.ip_stack),
            build_traffic_selector_responder_payload(self.profile.epdg.ip_stack),
            build_notify_payload(
                NotifyProtocolId::None,
                &[],
                NOTIFY_EAP_ONLY_AUTHENTICATION,
                &[],
            )?,
        ];
        if self.profile.ikev2.include_epdg_idr {
            let responder_identity = self.profile.epdg.apn.unwrap_or(self.profile.epdg.host);
            payloads.insert(
                1,
                build_identification_responder_payload(responder_identity),
            );
        }
        Ok(payloads)
    }

    fn child_sa_initiator_spi(&self) -> Result<[u8; 4], IkeStateError> {
        if let Some(spi) = self.private.child_sa_initiator_spi {
            return Ok(spi);
        }
        let mut seed = Vec::with_capacity(
            self.private.initiator_nonce.len()
                + self
                    .private
                    .responder_nonce
                    .as_ref()
                    .map(|nonce| nonce.len())
                    .unwrap_or_default()
                + 8,
        );
        seed.extend_from_slice(&self.private.initiator_spi.to_be_bytes());
        seed.extend_from_slice(&self.private.initiator_nonce);
        if let Some(responder_nonce) = &self.private.responder_nonce {
            seed.extend_from_slice(responder_nonce);
        }
        let digest = ring::digest::digest(&ring::digest::SHA256, &seed);
        let mut spi = [0u8; 4];
        spi.copy_from_slice(&digest.as_ref()[..4]);
        if spi == [0, 0, 0, 0] {
            spi[3] = 1;
        }
        Ok(spi)
    }

    pub fn accept_eap_aka_challenge(&mut self, eap_packet: &[u8]) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::AuthRequestBuilt)?;
        let summary = parse_eap_aka_summary(eap_packet)?;
        if summary.code != "request" || summary.method != "aka" || summary.subtype != "challenge" {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "expected_eap_aka_challenge",
            });
        }

        self.eap = Some(summary);
        self.phase = IkeStatePhase::EapAkaChallengeReceived;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "inbound",
            payloads: vec!["eap_aka"],
            note: "eap_aka_challenge_received",
        });
        self.phase = IkeStatePhase::UsimAkaResponsePending;
        self.next_message_id = self.next_message_id.saturating_add(1);
        Ok(())
    }

    pub fn accept_encrypted_eap_aka_challenge(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::AuthRequestBuilt)?;
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let payloads = decrypt_encrypted_payload_from_message(
            encoded_message,
            bundle,
            IkeSkDirection::ResponderToInitiator,
        )?;
        let eap_payload = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::ExtensibleAuthentication)
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_eap_payload",
            })?;
        let summary = parse_eap_aka_summary(&eap_payload.body)?;
        if summary.code != "request" || summary.method != "aka" || summary.subtype != "challenge" {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "expected_eap_aka_challenge",
            });
        }

        self.eap = Some(summary);
        self.phase = IkeStatePhase::EapAkaChallengeReceived;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "inbound",
            payloads: vec!["eap_aka"],
            note: "encrypted_eap_aka_challenge_received",
        });
        self.phase = IkeStatePhase::UsimAkaResponsePending;
        self.next_message_id = self.next_message_id.saturating_add(1);
        Ok(())
    }

    pub fn decrypted_eap_aka_challenge_packet(
        &self,
        encoded_message: &[u8],
    ) -> Result<Vec<u8>, IkeStateError> {
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let payloads = decrypt_encrypted_payload_from_message(
            encoded_message,
            bundle,
            IkeSkDirection::ResponderToInitiator,
        )?;
        let eap_payload = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::ExtensibleAuthentication)
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_eap_payload",
            })?;
        Ok(eap_payload.body.clone())
    }

    pub fn build_encrypted_eap_response_packet(
        &mut self,
        eap_response: &[u8],
    ) -> Result<Vec<u8>, IkeStateError> {
        if !matches!(
            self.phase,
            IkeStatePhase::UsimAkaResponsePending
                | IkeStatePhase::EapAkaIdentityReceived
                | IkeStatePhase::EapAkaNotificationReceived
        ) {
            return Err(IkeStateError::InvalidPhase {
                expected: "usim_aka_response_pending_or_eap_aka_identity_or_notification_received",
                actual: self.phase.as_str(),
            });
        }
        let responder_spi = self
            .private
            .responder_spi
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            })?;
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let inner_payloads = vec![IkePayload {
            payload_type: IkePayloadType::ExtensibleAuthentication,
            critical: false,
            body: eap_response.to_vec(),
        }];
        let first_inner_payload = inner_payloads[0].payload_type;
        let encrypted_payload = build_encrypted_payload(
            bundle,
            IkeSkDirection::InitiatorToResponder,
            &inner_payloads,
        )?;
        let message = encrypted_message_from_payload(
            self.private.initiator_spi,
            responder_spi,
            IkeExchangeType::IkeAuth,
            true,
            self.next_message_id,
            encrypted_payload,
        );
        let packet = encode_encrypted_message(
            &message,
            first_inner_payload,
            bundle,
            IkeSkDirection::InitiatorToResponder,
        )?;
        self.phase = IkeStatePhase::EapAkaResponseReady;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "outbound",
            payloads: vec!["eap_aka"],
            note: "encrypted_eap_aka_response_built",
        });
        self.next_message_id = self.next_message_id.saturating_add(1);
        Ok(packet)
    }

    pub fn next_message_id(&self) -> u32 {
        self.next_message_id
    }

    pub fn accept_encrypted_eap_aka_challenge_reason(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), String> {
        let inner_payload_reason = self
            .encrypted_inner_payload_reason(encoded_message)
            .unwrap_or_else(|_| "missing_eap_payload".to_string());
        self.accept_encrypted_eap_aka_challenge(encoded_message)
            .map_err(|err| match err {
                IkeStateError::EncryptedPayload(EncryptedPayloadError::IntegrityMismatch) => {
                    "ike_auth_eap_integrity_mismatch".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidEncryptedPayload) => {
                    "ike_auth_eap_invalid_encrypted_payload".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidPadding) => {
                    "ike_auth_eap_invalid_padding".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::UnsupportedCipher(_)) => {
                    "ike_auth_eap_unsupported_cipher".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::UnsupportedIntegrity(_)) => {
                    "ike_auth_eap_unsupported_integrity".to_string()
                }
                IkeStateError::Eap(_) => "ike_auth_eap_packet_parse_failed".to_string(),
                IkeStateError::InvalidResponse {
                    reason: "missing_eap_payload",
                } => inner_payload_reason,
                IkeStateError::InvalidResponse { reason } => reason.to_string(),
                _ => "ike_auth_eap_challenge_decode_failed".to_string(),
            })
    }

    fn encrypted_inner_payload_reason(
        &self,
        encoded_message: &[u8],
    ) -> Result<String, IkeStateError> {
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let payloads = decrypt_encrypted_payload_from_message(
            encoded_message,
            bundle,
            IkeSkDirection::ResponderToInitiator,
        )?;
        Ok(match payloads.first().map(|payload| payload.payload_type) {
            Some(IkePayloadType::Notify) => payloads
                .first()
                .and_then(|payload| notify_reason(&payload.body))
                .unwrap_or_else(|| "ike_auth_inner_notify".to_string()),
            Some(IkePayloadType::IdentificationResponder) => "ike_auth_inner_idr".to_string(),
            Some(IkePayloadType::CertificateRequest) => "ike_auth_inner_certreq".to_string(),
            Some(IkePayloadType::Authentication) => "ike_auth_inner_auth".to_string(),
            Some(IkePayloadType::Configuration) => "ike_auth_inner_configuration".to_string(),
            Some(IkePayloadType::TrafficSelectorInitiator) => "ike_auth_inner_tsi".to_string(),
            Some(IkePayloadType::TrafficSelectorResponder) => "ike_auth_inner_tsr".to_string(),
            Some(IkePayloadType::ExtensibleAuthentication) => "ike_auth_inner_eap".to_string(),
            Some(_) => "ike_auth_inner_other".to_string(),
            None => "ike_auth_inner_empty".to_string(),
        })
    }

    pub fn mark_usim_aka_response_ready(&mut self) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::UsimAkaResponsePending)?;
        self.phase = IkeStatePhase::EapAkaResponseReady;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "local",
            payloads: vec!["usim_aka_response_summary"],
            note: "usim_aka_response_ready",
        });
        Ok(())
    }

    pub fn accept_auth_success(&mut self, eap_packet: &[u8]) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::EapAkaResponseReady)?;
        let summary = parse_eap_aka_summary(eap_packet)?;
        if summary.code != "success" {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "expected_eap_success",
            });
        }

        self.eap = Some(summary);
        self.phase = IkeStatePhase::AuthSuccessAccepted;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "inbound",
            payloads: vec!["eap_success"],
            note: "auth_success_accepted",
        });
        Ok(())
    }

    pub fn accept_encrypted_auth_success_or_reason(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), String> {
        self.accept_encrypted_auth_success(encoded_message)
            .map_err(|err| match err {
                IkeStateError::EncryptedPayload(EncryptedPayloadError::IntegrityMismatch) => {
                    "ike_auth_final_integrity_mismatch".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidEncryptedPayload) => {
                    "ike_auth_final_invalid_encrypted_payload".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidPadding) => {
                    "ike_auth_final_invalid_padding".to_string()
                }
                IkeStateError::Eap(_) => "ike_auth_final_eap_parse_failed".to_string(),
                IkeStateError::InvalidResponse {
                    reason: "missing_eap_payload",
                } => self
                    .encrypted_inner_payload_reason(encoded_message)
                    .unwrap_or_else(|_| "ike_auth_final_missing_eap".to_string()),
                IkeStateError::InvalidResponse { reason } => reason.to_string(),
                _ => "ike_auth_final_decode_failed".to_string(),
            })
    }

    pub fn accept_encrypted_auth_success(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), IkeStateError> {
        match self.accept_encrypted_auth_progress(encoded_message)? {
            IkeAuthProgress::EapSuccess { .. } => Ok(()),
            IkeAuthProgress::EapAkaIdentity { .. } => self.fail(IkeStateError::InvalidResponse {
                reason: "expected_eap_success",
            }),
            IkeAuthProgress::EapAkaNotification { .. } => {
                self.fail(IkeStateError::InvalidResponse {
                    reason: "expected_eap_success",
                })
            }
        }
    }

    pub fn accept_encrypted_auth_progress_or_reason(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<IkeAuthProgress, String> {
        self.accept_encrypted_auth_progress(encoded_message)
            .map_err(|err| match err {
                IkeStateError::EncryptedPayload(EncryptedPayloadError::IntegrityMismatch) => {
                    "ike_auth_final_integrity_mismatch".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidEncryptedPayload) => {
                    "ike_auth_final_invalid_encrypted_payload".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidPadding) => {
                    "ike_auth_final_invalid_padding".to_string()
                }
                IkeStateError::Eap(_) => "ike_auth_final_eap_parse_failed".to_string(),
                IkeStateError::InvalidResponse {
                    reason: "missing_eap_payload",
                } => self
                    .encrypted_inner_payload_reason(encoded_message)
                    .unwrap_or_else(|_| "ike_auth_final_missing_eap".to_string()),
                IkeStateError::InvalidResponse { reason } => reason.to_string(),
                _ => "ike_auth_final_decode_failed".to_string(),
            })
    }

    pub fn accept_encrypted_auth_progress(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<IkeAuthProgress, IkeStateError> {
        self.require_phase(IkeStatePhase::EapAkaResponseReady)?;
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let payloads = decrypt_encrypted_payload_from_message(
            encoded_message,
            bundle,
            IkeSkDirection::ResponderToInitiator,
        )?;
        let eap_payload = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::ExtensibleAuthentication)
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_eap_payload",
            })?;
        let summary = parse_eap_aka_summary(&eap_payload.body)?;
        if summary.code == "request" && summary.method == "aka" && summary.subtype == "identity" {
            self.eap = Some(summary);
            self.phase = IkeStatePhase::EapAkaIdentityReceived;
            self.transcript.push(IkeTranscriptEvent {
                message_id: self.next_message_id,
                exchange: "ike_auth",
                direction: "inbound",
                payloads: payloads
                    .iter()
                    .map(|payload| payload.payload_type.name())
                    .collect(),
                note: "encrypted_eap_aka_identity_received",
            });
            return Ok(IkeAuthProgress::EapAkaIdentity {
                packet: eap_payload.body.clone(),
            });
        }
        if summary.code == "request" && summary.method == "aka" && summary.subtype == "notification"
        {
            self.eap = Some(summary);
            self.phase = IkeStatePhase::EapAkaNotificationReceived;
            self.transcript.push(IkeTranscriptEvent {
                message_id: self.next_message_id,
                exchange: "ike_auth",
                direction: "inbound",
                payloads: payloads
                    .iter()
                    .map(|payload| payload.payload_type.name())
                    .collect(),
                note: "encrypted_eap_aka_notification_received",
            });
            return Ok(IkeAuthProgress::EapAkaNotification {
                packet: eap_payload.body.clone(),
            });
        }
        if summary.code != "success" {
            return self.fail(IkeStateError::InvalidResponse {
                reason: "expected_eap_success",
            });
        }

        self.eap = Some(summary);
        let child_sa_included = payloads
            .iter()
            .any(|payload| payload.payload_type == IkePayloadType::SecurityAssociation);
        if child_sa_included {
            let child_sa = self.summarize_child_sa_payloads(&payloads)?;
            self.child_sa = Some(child_sa);
            self.phase = IkeStatePhase::ChildSaReady;
        } else {
            self.phase = IkeStatePhase::AuthSuccessAccepted;
        }
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "inbound",
            payloads: payloads
                .iter()
                .map(|payload| payload.payload_type.name())
                .collect(),
            note: if child_sa_included {
                "encrypted_auth_success_child_sa_accepted"
            } else {
                "encrypted_auth_success_accepted"
            },
        });
        Ok(IkeAuthProgress::EapSuccess { child_sa_included })
    }

    pub fn build_encrypted_final_auth_packet(
        &mut self,
        eap_msk: &[u8],
    ) -> Result<Vec<u8>, IkeStateError> {
        self.require_phase(IkeStatePhase::AuthSuccessAccepted)?;
        let responder_spi = self
            .private
            .responder_spi
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_responder_spi",
            })?;
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let authentication_data = self.initiator_auth_data(eap_msk)?;
        let inner_payloads = vec![build_authentication_shared_key_payload(
            &authentication_data,
        )];
        let first_inner_payload = inner_payloads[0].payload_type;
        let encrypted_payload = build_encrypted_payload(
            bundle,
            IkeSkDirection::InitiatorToResponder,
            &inner_payloads,
        )?;
        let message = encrypted_message_from_payload(
            self.private.initiator_spi,
            responder_spi,
            IkeExchangeType::IkeAuth,
            true,
            self.next_message_id,
            encrypted_payload,
        );
        let packet = encode_encrypted_message(
            &message,
            first_inner_payload,
            bundle,
            IkeSkDirection::InitiatorToResponder,
        )?;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "outbound",
            payloads: vec!["authentication"],
            note: "final_auth_request_built",
        });
        self.next_message_id = self.next_message_id.saturating_add(1);
        Ok(packet)
    }

    pub fn accept_encrypted_child_sa_response_or_reason(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), String> {
        self.accept_encrypted_child_sa_response(encoded_message)
            .map_err(|err| match err {
                IkeStateError::EncryptedPayload(EncryptedPayloadError::IntegrityMismatch) => {
                    "ike_auth_child_sa_integrity_mismatch".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidEncryptedPayload) => {
                    "ike_auth_child_sa_invalid_encrypted_payload".to_string()
                }
                IkeStateError::EncryptedPayload(EncryptedPayloadError::InvalidPadding) => {
                    "ike_auth_child_sa_invalid_padding".to_string()
                }
                IkeStateError::InvalidResponse {
                    reason: "missing_child_sa_payload",
                } => self
                    .encrypted_inner_payload_reason(encoded_message)
                    .unwrap_or_else(|_| "missing_child_sa_payload".to_string()),
                IkeStateError::InvalidResponse { reason } => reason.to_string(),
                _ => "ike_auth_child_sa_decode_failed".to_string(),
            })
    }

    pub fn accept_encrypted_child_sa_response(
        &mut self,
        encoded_message: &[u8],
    ) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::AuthSuccessAccepted)?;
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let payloads = decrypt_encrypted_payload_from_message(
            encoded_message,
            bundle,
            IkeSkDirection::ResponderToInitiator,
        )?;
        let summary = self.summarize_child_sa_payloads(&payloads)?;
        self.child_sa = Some(summary);
        self.phase = IkeStatePhase::ChildSaReady;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "inbound",
            payloads: payloads
                .iter()
                .map(|payload| payload.payload_type.name())
                .collect(),
            note: "child_sa_response_accepted",
        });
        Ok(())
    }

    pub fn mark_child_sa_ready(&mut self) -> Result<(), IkeStateError> {
        self.require_phase(IkeStatePhase::AuthSuccessAccepted)?;
        self.phase = IkeStatePhase::ChildSaReady;
        self.transcript.push(IkeTranscriptEvent {
            message_id: self.next_message_id,
            exchange: "ike_auth",
            direction: "local",
            payloads: vec!["child_sa_summary"],
            note: "child_sa_ready",
        });
        Ok(())
    }

    pub fn child_sa_material(&self) -> Option<&IkeChildSaMaterial> {
        self.private.child_sa_material.as_ref()
    }

    pub fn handle_control_message(&mut self, encoded: &[u8]) -> Result<(), IkeStateError> {
        let message = IkeMessage::decode(encoded)?;
        let event = parse_control_event(&message)?;
        let action = event.action;
        self.last_control_event = Some(event);
        self.push_event(&message, "inbound", action);

        match action {
            "fail_exchange" => self.fail(IkeStateError::InvalidResponse {
                reason: "control_event_failed",
            }),
            "mark_child_sa_missing" => {
                self.phase = IkeStatePhase::ChildSaMissing;
                Ok(())
            }
            "reauth_required" => {
                self.phase = IkeStatePhase::ReauthRequired;
                Ok(())
            }
            "schedule_rekey" => {
                self.phase = IkeStatePhase::RekeyPending;
                Ok(())
            }
            "teardown_requested" => {
                self.phase = IkeStatePhase::TeardownRequested;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn require_phase(&self, expected: IkeStatePhase) -> Result<(), IkeStateError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(IkeStateError::InvalidPhase {
                expected: expected.as_str(),
                actual: self.phase.as_str(),
            })
        }
    }

    fn push_event(&mut self, message: &IkeMessage, direction: &'static str, note: &'static str) {
        self.transcript.push(IkeTranscriptEvent {
            message_id: message.header.message_id,
            exchange: message.header.exchange_type.name(),
            direction,
            payloads: message
                .payloads
                .iter()
                .map(|payload| payload.payload_type.name())
                .collect(),
            note,
        });
    }

    fn initiator_auth_data(&self, eap_msk: &[u8]) -> Result<Vec<u8>, IkeStateError> {
        if eap_msk.is_empty() {
            return Err(IkeStateError::InvalidResponse {
                reason: "missing_eap_msk",
            });
        }
        let bundle = self
            .private
            .secret_bundle
            .as_ref()
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_session_keys",
            })?;
        let sa_init_request =
            self.private
                .sa_init_request_packet
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_sa_init_request",
                })?;
        let responder_nonce =
            self.private
                .responder_nonce
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_responder_nonce",
                })?;
        let idi_body = self.private.initiator_id_payload_body.as_ref().ok_or(
            IkeStateError::InvalidResponse {
                reason: "missing_initiator_identity_payload",
            },
        )?;
        let idi_hash = bundle.prf_bytes(bundle.sk_pi.expose_for_protocol(), idi_body)?;
        let mut signed_octets =
            Vec::with_capacity(sa_init_request.len() + responder_nonce.len() + idi_hash.len());
        signed_octets.extend_from_slice(sa_init_request);
        signed_octets.extend_from_slice(responder_nonce);
        signed_octets.extend_from_slice(&idi_hash);
        let shared_key = bundle.prf_bytes(eap_msk, b"Key Pad for IKEv2")?;
        bundle
            .prf_bytes(&shared_key, &signed_octets)
            .map_err(IkeStateError::from)
    }

    fn summarize_child_sa_payloads(
        &mut self,
        payloads: &[IkePayload],
    ) -> Result<IkeChildSaSummary, IkeStateError> {
        let auth_present = payloads
            .iter()
            .any(|payload| payload.payload_type == IkePayloadType::Authentication);

        let sa_payload = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::SecurityAssociation)
            .ok_or(IkeStateError::InvalidResponse {
                reason: "missing_child_sa_payload",
            })?;
        let proposals = parse_sa_payload(&sa_payload.body)?;
        let expected_spi =
            self.private
                .child_sa_initiator_spi
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_child_sa_initiator_spi",
                })?;
        let selected = proposals
            .into_iter()
            .find(|proposal| {
                proposal.protocol_id == IkeProtocolId::Esp.as_u8()
                    && proposal.spi.len() == 4
                    && child_sa_selection_matches_profile(proposal, self.profile)
            })
            .ok_or(IkeStateError::InvalidResponse {
                reason: "child_sa_selection_unacceptable",
            })?;
        let inbound_sa_identifier = u32::from_be_bytes(expected_spi);
        let outbound_sa_identifier =
            u32::from_be_bytes(selected.spi.as_slice().try_into().map_err(|_| {
                IkeStateError::InvalidResponse {
                    reason: "invalid_child_sa_responder_spi",
                }
            })?);
        if inbound_sa_identifier == 0 || outbound_sa_identifier == 0 {
            return Err(IkeStateError::InvalidResponse {
                reason: "invalid_child_sa_spi",
            });
        }
        let selected_profile_proposal = matching_child_sa_profile_proposal(&selected, self.profile)
            .ok_or(IkeStateError::InvalidResponse {
                reason: "child_sa_selection_unacceptable",
            })?;
        let secret_bundle =
            self.private
                .secret_bundle
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_session_keys",
                })?;
        let responder_nonce =
            self.private
                .responder_nonce
                .as_ref()
                .ok_or(IkeStateError::InvalidResponse {
                    reason: "missing_responder_nonce",
                })?;
        let esp_proposal =
            child_sa_proposal_from_profile_string(selected_profile_proposal, 1, &selected.spi)?;
        let secrets = derive_child_sa_secret_pair(
            secret_bundle,
            &esp_proposal,
            &self.private.initiator_nonce,
            responder_nonce,
        )?;

        let config_summary = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::Configuration)
            .map(|payload| summarize_configuration_reply(&payload.body))
            .transpose()?;
        let traffic_selectors_present = payloads
            .iter()
            .any(|payload| payload.payload_type == IkePayloadType::TrafficSelectorInitiator)
            && payloads
                .iter()
                .any(|payload| payload.payload_type == IkePayloadType::TrafficSelectorResponder);

        self.private.child_sa_material = Some(IkeChildSaMaterial {
            inbound_sa_identifier,
            outbound_sa_identifier,
            selected_profile_proposal,
            proposal: selected,
            configuration: config_summary
                .as_ref()
                .map(|summary| IkeConfigurationMaterial {
                    assigned_inner_addresses: summary.assigned_inner_addresses.clone(),
                    assigned_ipv6_prefix_length: summary.assigned_ipv6_prefix_length,
                    pcscf_addresses: summary.pcscf_addresses.clone(),
                    dns_addresses: summary.dns_addresses.clone(),
                }),
            secrets,
        });

        Ok(IkeChildSaSummary {
            responder_auth_present: auth_present,
            child_sa_present: true,
            selected_esp_proposal_present: true,
            inbound_sa_identifier_present: true,
            outbound_sa_identifier_present: true,
            configuration_reply_present: config_summary.is_some(),
            assigned_inner_address_present: config_summary
                .as_ref()
                .map(|summary| summary.assigned_inner_address_present)
                .unwrap_or(false),
            assigned_inner_address_count: config_summary
                .as_ref()
                .map(|summary| summary.assigned_inner_addresses.len())
                .unwrap_or_default(),
            assigned_ipv6_prefix_length_present: config_summary
                .as_ref()
                .and_then(|summary| summary.assigned_ipv6_prefix_length)
                .is_some(),
            pcscf_present: config_summary
                .as_ref()
                .map(|summary| summary.pcscf_present)
                .unwrap_or(false),
            pcscf_count: config_summary
                .as_ref()
                .map(|summary| summary.pcscf_addresses.len())
                .unwrap_or_default(),
            dns_present: config_summary
                .as_ref()
                .map(|summary| summary.dns_present)
                .unwrap_or(false),
            dns_count: config_summary
                .as_ref()
                .map(|summary| summary.dns_addresses.len())
                .unwrap_or_default(),
            traffic_selectors_present,
            sensitive_values_policy: "child_sa_identifiers_addresses_and_selectors_not_serialized",
        })
    }

    fn fail<T>(&mut self, error: IkeStateError) -> Result<T, IkeStateError> {
        self.phase = IkeStatePhase::Failed;
        self.last_error = Some(error.to_string());
        Err(error)
    }
}

fn notify_reason(body: &[u8]) -> Option<String> {
    let notify_type = match notify_type(body) {
        Some(notify_type) => notify_type,
        None => {
            return Some("ike_auth_notify_truncated".to_string());
        }
    };
    Some(
        match super::ike_events::notify_name(notify_type) {
            "authentication_failed" => "ike_auth_notify_authentication_failed",
            "no_proposal_chosen" => "ike_auth_notify_no_proposal_chosen",
            "invalid_syntax" => "ike_auth_notify_invalid_syntax",
            "invalid_ke_payload" => "ike_auth_notify_invalid_ke_payload",
            "invalid_group_id" => "ike_auth_notify_invalid_group_id",
            "single_pair_required" => "ike_auth_notify_single_pair_required",
            "no_additional_sas" => "ike_auth_notify_no_additional_sas",
            "internal_address_failure" => "ike_auth_notify_internal_address_failure",
            "failed_cp_required" => "ike_auth_notify_failed_cp_required",
            "ts_unacceptable" => "ike_auth_notify_ts_unacceptable",
            "invalid_selectors" => "ike_auth_notify_invalid_selectors",
            "unacceptable_addresses" => "ike_auth_notify_unacceptable_addresses",
            "temporary_failure" => "ike_auth_notify_temporary_failure",
            "child_sa_not_found" => "ike_auth_notify_child_sa_not_found",
            "authorization_failed" => "ike_auth_notify_authorization_failed",
            "private_or_extension" => return Some(notify_private_or_extension_reason(notify_type)),
            _ => "ike_auth_notify_unknown",
        }
        .to_string(),
    )
}

fn notify_private_or_extension_reason(notify_type: u16) -> String {
    match notify_type {
        8_192..=16_383 => {
            format!("ike_auth_notify_status_reserved_or_extension_type_{notify_type}")
        }
        40_960..=65_535 => format!("ike_auth_notify_private_use_type_{notify_type}"),
        _ => format!("ike_auth_notify_private_or_extension_type_{notify_type}"),
    }
}

fn notify_type(body: &[u8]) -> Option<u16> {
    if body.len() < 4 {
        None
    } else {
        Some(u16::from_be_bytes([body[2], body[3]]))
    }
}

fn nat_detection_hash(initiator_spi: u64, responder_spi: u64, addr: SocketAddr) -> [u8; 20] {
    let mut input = Vec::new();
    input.extend_from_slice(&initiator_spi.to_be_bytes());
    input.extend_from_slice(&responder_spi.to_be_bytes());
    match addr {
        SocketAddr::V4(v4) => input.extend_from_slice(&v4.ip().octets()),
        SocketAddr::V6(v6) => input.extend_from_slice(&v6.ip().octets()),
    }
    input.extend_from_slice(&addr.port().to_be_bytes());
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);
    digest.as_ref().try_into().expect("sha1 digest length")
}

fn find_payload(
    message: &IkeMessage,
    payload_type: IkePayloadType,
) -> Option<&super::ike_codec::IkePayload> {
    message
        .payloads
        .iter()
        .find(|payload| payload.payload_type == payload_type)
}

fn matching_ike_profile_proposal(
    proposals: &[ParsedProposal],
    offered_proposals: &[&'static str],
) -> Option<&'static str> {
    offered_proposals
        .iter()
        .enumerate()
        .find_map(|(index, proposal_text)| {
            let expected =
                ike_proposal_from_profile_string(proposal_text, (index + 1) as u8).ok()?;
            proposals
                .iter()
                .any(|proposal| {
                    proposal.protocol_id == expected.protocol_id.as_u8()
                        && expected.transforms.iter().all(|expected_transform| {
                            proposal.transforms.iter().any(|parsed| {
                                parsed.transform_type == expected_transform.transform_type.as_u8()
                                    && parsed.transform_id == expected_transform.transform_id
                                    && transform_attributes_match(parsed, expected_transform)
                            })
                        })
                })
                .then_some(*proposal_text)
        })
}

fn child_sa_selection_matches_profile(
    proposal: &ParsedProposal,
    profile: &'static CarrierProfile,
) -> bool {
    matching_child_sa_profile_proposal(proposal, profile).is_some()
}

fn matching_child_sa_profile_proposal(
    proposal: &ParsedProposal,
    profile: &'static CarrierProfile,
) -> Option<&'static str> {
    profile
        .ikev2
        .esp_proposals
        .iter()
        .copied()
        .find(|proposal_text| {
            child_sa_proposal_from_profile_string(proposal_text, proposal.number, &proposal.spi)
                .map(|expected| {
                    expected.transforms.iter().all(|expected_transform| {
                        proposal.transforms.iter().any(|parsed| {
                            parsed.transform_type == expected_transform.transform_type.as_u8()
                                && parsed.transform_id == expected_transform.transform_id
                                && transform_attributes_match(parsed, expected_transform)
                        })
                    })
                })
                .unwrap_or(false)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigurationReplySummary {
    assigned_inner_address_present: bool,
    assigned_inner_addresses: Vec<IpAddr>,
    assigned_ipv6_prefix_length: Option<u8>,
    pcscf_present: bool,
    pcscf_addresses: Vec<IpAddr>,
    dns_present: bool,
    dns_addresses: Vec<IpAddr>,
}

fn summarize_configuration_reply(body: &[u8]) -> Result<ConfigurationReplySummary, IkeStateError> {
    if body.len() < 4 {
        return Err(IkeStateError::InvalidResponse {
            reason: "invalid_configuration_payload",
        });
    }
    let mut summary = ConfigurationReplySummary {
        assigned_inner_address_present: false,
        assigned_inner_addresses: Vec::new(),
        assigned_ipv6_prefix_length: None,
        pcscf_present: false,
        pcscf_addresses: Vec::new(),
        dns_present: false,
        dns_addresses: Vec::new(),
    };
    let mut input = &body[4..];
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(IkeStateError::InvalidResponse {
                reason: "invalid_configuration_attribute",
            });
        }
        let attribute_type = u16::from_be_bytes([input[0], input[1]]) & 0x7fff;
        let len = u16::from_be_bytes([input[2], input[3]]) as usize;
        if input.len() < 4 + len {
            return Err(IkeStateError::InvalidResponse {
                reason: "invalid_configuration_attribute",
            });
        }
        let value = &input[4..4 + len];
        match attribute_type {
            CFG_ATTR_INTERNAL_IP4_ADDRESS => {
                summary.assigned_inner_address_present = true;
                if let Some(addr) = parse_ipv4_config_value(value) {
                    summary.assigned_inner_addresses.push(IpAddr::V4(addr));
                }
            }
            CFG_ATTR_INTERNAL_IP6_ADDRESS => {
                summary.assigned_inner_address_present = true;
                if let Some((addr, prefix_len)) = parse_ipv6_config_value(value) {
                    summary.assigned_inner_addresses.push(IpAddr::V6(addr));
                    summary.assigned_ipv6_prefix_length = Some(prefix_len);
                }
            }
            CFG_ATTR_INTERNAL_IP4_DNS => {
                summary.dns_present = true;
                if let Some(addr) = parse_ipv4_config_value(value) {
                    summary.dns_addresses.push(IpAddr::V4(addr));
                }
            }
            CFG_ATTR_INTERNAL_IP6_DNS => {
                summary.dns_present = true;
                if let Some((addr, _prefix_len)) = parse_ipv6_config_value(value) {
                    summary.dns_addresses.push(IpAddr::V6(addr));
                }
            }
            CFG_ATTR_INTERNAL_IP4_PCSCF => {
                summary.pcscf_present = true;
                if let Some(addr) = parse_ipv4_config_value(value) {
                    summary.pcscf_addresses.push(IpAddr::V4(addr));
                }
            }
            CFG_ATTR_INTERNAL_IP6_PCSCF => {
                summary.pcscf_present = true;
                if let Some((addr, _prefix_len)) = parse_ipv6_config_value(value) {
                    summary.pcscf_addresses.push(IpAddr::V6(addr));
                }
            }
            _ => {}
        }
        input = &input[4 + len..];
    }
    Ok(summary)
}

fn parse_ipv4_config_value(value: &[u8]) -> Option<Ipv4Addr> {
    if value.len() != 4 {
        return None;
    }
    Some(Ipv4Addr::new(value[0], value[1], value[2], value[3]))
}

fn parse_ipv6_config_value(value: &[u8]) -> Option<(Ipv6Addr, u8)> {
    if value.len() == 16 {
        return Some((Ipv6Addr::from(<[u8; 16]>::try_from(value).ok()?), 128));
    }
    if value.len() == 17 {
        let addr = Ipv6Addr::from(<[u8; 16]>::try_from(&value[..16]).ok()?);
        return Some((addr, value[16]));
    }
    None
}

fn transform_attributes_match(
    parsed: &super::ike_payloads::ParsedTransform,
    expected: &super::ike_payloads::TransformSpec,
) -> bool {
    expected.attributes.iter().all(|attribute| match attribute {
        super::ike_payloads::TransformAttribute::KeyLength(bits) => parsed
            .attributes
            .iter()
            .any(|parsed_attr| parsed_attr.attribute_type == 14 && parsed_attr.value == *bits),
    })
}

fn encrypted_inner_identity_placeholder() -> super::ike_codec::IkePayload {
    super::ike_codec::IkePayload {
        payload_type: IkePayloadType::IdentificationInitiator,
        critical: false,
        body: b"simadmin-inner-identity-metadata".to_vec(),
    }
}

trait IkeMessageExt {
    fn with_responder_spi(self, responder_spi: u64) -> Self;
}

impl IkeMessageExt for IkeMessage {
    fn with_responder_spi(mut self, responder_spi: u64) -> Self {
        self.header.responder_spi = responder_spi;
        self
    }
}

trait IkeExchangeTypeName {
    fn name(self) -> &'static str;
}

impl IkeExchangeTypeName for IkeExchangeType {
    fn name(self) -> &'static str {
        match self {
            Self::IkeSaInit => "ike_sa_init",
            Self::IkeAuth => "ike_auth",
            Self::CreateChildSa => "create_child_sa",
            Self::Informational => "informational",
            Self::SessionResume => "session_resume",
            Self::Unknown(_) => "unknown",
        }
    }
}

trait IkePayloadTypeName {
    fn name(self) -> &'static str;
}

impl IkePayloadTypeName for IkePayloadType {
    fn name(self) -> &'static str {
        match self {
            Self::NoNext => "no_next",
            Self::SecurityAssociation => "security_association",
            Self::KeyExchange => "key_exchange",
            Self::IdentificationInitiator => "identification_initiator",
            Self::IdentificationResponder => "identification_responder",
            Self::Certificate => "certificate",
            Self::CertificateRequest => "certificate_request",
            Self::Authentication => "authentication",
            Self::Nonce => "nonce",
            Self::Notify => "notify",
            Self::Delete => "delete",
            Self::VendorId => "vendor_id",
            Self::TrafficSelectorInitiator => "traffic_selector_initiator",
            Self::TrafficSelectorResponder => "traffic_selector_responder",
            Self::Encrypted => "encrypted",
            Self::Configuration => "configuration",
            Self::ExtensibleAuthentication => "extensible_authentication",
            Self::EncryptedFragment => "encrypted_fragment",
            Self::Unknown(_) => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::{
        ike_codec::{IkeFlags, IkeHeader, IkePayload},
        ike_encrypted::{
            build_encrypted_payload, encode_encrypted_message, encrypted_message_from_payload,
            IkeSkDirection,
        },
        profiles::{GB_EE_23433, NL_VODAFONE_20404},
    };

    fn machine(profile: &'static CarrierProfile) -> IkeStateMachine {
        IkeStateMachine::new(
            profile,
            0x1111_2222_3333_4444,
            vec![0x55; 32],
            vec![0x66; 256],
        )
    }

    fn sa_init_response(request: &IkeMessage) -> Vec<u8> {
        let sa_body = request
            .payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::SecurityAssociation)
            .expect("request has SA")
            .body
            .clone();
        IkeMessage {
            header: IkeHeader {
                initiator_spi: request.header.initiator_spi,
                responder_spi: 0x9999_aaaa_bbbb_cccc,
                next_payload: IkePayloadType::SecurityAssociation,
                major_version: 2,
                minor_version: 0,
                exchange_type: IkeExchangeType::IkeSaInit,
                flags: IkeFlags {
                    initiator: false,
                    response: true,
                    version: false,
                },
                message_id: 0,
                length: 0,
            },
            payloads: vec![
                IkePayload {
                    payload_type: IkePayloadType::SecurityAssociation,
                    critical: false,
                    body: sa_body,
                },
                IkePayload {
                    payload_type: IkePayloadType::KeyExchange,
                    critical: false,
                    body: build_ke_payload(DH_MODP_2048, &[0x88; 256]).body,
                },
                IkePayload {
                    payload_type: IkePayloadType::Nonce,
                    critical: false,
                    body: vec![0x99; 32],
                },
            ],
        }
        .encode()
        .expect("encode response")
    }

    fn sa_init_response_with_sa_body(request: &IkeMessage, sa_body: Vec<u8>) -> Vec<u8> {
        IkeMessage {
            header: IkeHeader {
                initiator_spi: request.header.initiator_spi,
                responder_spi: 0x9999_aaaa_bbbb_cccc,
                next_payload: IkePayloadType::SecurityAssociation,
                major_version: 2,
                minor_version: 0,
                exchange_type: IkeExchangeType::IkeSaInit,
                flags: IkeFlags {
                    initiator: false,
                    response: true,
                    version: false,
                },
                message_id: 0,
                length: 0,
            },
            payloads: vec![
                IkePayload {
                    payload_type: IkePayloadType::SecurityAssociation,
                    critical: false,
                    body: sa_body,
                },
                IkePayload {
                    payload_type: IkePayloadType::KeyExchange,
                    critical: false,
                    body: build_ke_payload(DH_MODP_2048, &[0x88; 256]).body,
                },
                IkePayload {
                    payload_type: IkePayloadType::Nonce,
                    critical: false,
                    body: vec![0x99; 32],
                },
            ],
        }
        .encode()
        .expect("encode response")
    }

    #[test]
    fn builds_profile_driven_sa_init_request_without_serializing_private_material() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");

        assert_eq!(request.header.exchange_type, IkeExchangeType::IkeSaInit);
        assert_eq!(request.header.message_id, 0);
        assert_eq!(request.payloads.len(), 3);
        assert_eq!(
            request.payloads[0].payload_type,
            IkePayloadType::SecurityAssociation
        );
        assert_eq!(machine.snapshot().phase, "sa_init_request_built");
        assert_eq!(
            machine.snapshot().selected_proposal,
            Some("aes128-sha256-modp2048")
        );

        let json = serde_json::to_string(&machine.snapshot()).expect("serialize snapshot");
        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "initiator_nonce",
            "responder_nonce",
            "public_dh",
            "private",
            "ck",
            "ik",
            "key_material",
            "payload_body",
            "raw_payload",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "snapshot must not contain a {forbidden_key} field"
            );
        }
    }

    #[test]
    fn builds_sa_init_with_nat_detection_payloads_when_addresses_are_known() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request_for_addresses(
                "192.0.2.10:500".parse().expect("source"),
                "198.51.100.20:500".parse().expect("destination"),
            )
            .expect("build sa init request");

        let notify_types = request
            .payloads
            .iter()
            .filter(|payload| payload.payload_type == IkePayloadType::Notify)
            .filter_map(|payload| notify_type(&payload.body))
            .collect::<Vec<_>>();

        assert!(notify_types.contains(&NOTIFY_NAT_DETECTION_SOURCE_IP));
        assert!(notify_types.contains(&NOTIFY_NAT_DETECTION_DESTINATION_IP));
        assert_eq!(request.header.message_id, 0);
    }

    #[test]
    fn accepts_sa_init_response_and_builds_auth_start() {
        let mut metadata_machine = machine(&NL_VODAFONE_20404);
        let request = metadata_machine
            .build_sa_init_request()
            .expect("build sa init request");

        metadata_machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        assert_eq!(
            metadata_machine.snapshot().phase,
            "sa_init_response_accepted"
        );
        assert!(metadata_machine.snapshot().responder_spi_present);
        assert_eq!(
            metadata_machine.snapshot().selected_proposal,
            Some("aes256-sha256-prfsha512-modp2048")
        );
        metadata_machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        assert_eq!(metadata_machine.snapshot().phase, "session_keys_ready");
        assert_eq!(
            metadata_machine
                .snapshot()
                .key_schedule
                .as_ref()
                .map(|plan| plan.prf),
            Some("hmac_sha512")
        );

        let auth = metadata_machine
            .build_auth_eap_start_request()
            .expect("build auth request");
        assert_eq!(auth.header.exchange_type, IkeExchangeType::IkeAuth);
        assert_eq!(auth.header.message_id, 1);
        assert_eq!(auth.header.responder_spi, 0x9999_aaaa_bbbb_cccc);
        assert_eq!(auth.payloads[0].payload_type, IkePayloadType::Encrypted);
        assert!(!auth.payloads[0].body.is_empty());
        assert_eq!(
            metadata_machine
                .snapshot()
                .encrypted_payload
                .as_ref()
                .map(|plan| plan.mode),
            Some("metadata_only")
        );

        let mut packet_machine = machine(&NL_VODAFONE_20404);
        let request = packet_machine
            .build_sa_init_request()
            .expect("build sa init request");
        packet_machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        packet_machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        let packet = packet_machine
            .build_auth_eap_start_packet()
            .expect("build encrypted auth packet");
        assert!(!packet.is_empty());
        assert_eq!(packet[16], IkePayloadType::Encrypted.as_u8());
        assert_eq!(packet_machine.snapshot().phase, "auth_request_built");

        let json = serde_json::to_string(&packet_machine.snapshot()).expect("serialize snapshot");
        for forbidden_key in [
            "sk_ai",
            "sk_ei",
            "ciphertext",
            "integrity_tag",
            "shared_secret",
        ] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn auth_init_uses_profile_apn_as_responder_identity_when_available() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");

        let payloads = machine
            .build_auth_init_payloads(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
                [1, 2, 3, 4],
            )
            .expect("auth init payloads");
        let idr = payloads
            .iter()
            .find(|payload| payload.payload_type == IkePayloadType::IdentificationResponder)
            .expect("IDr payload");

        assert_eq!(idr.body[0], super::super::ike_payloads::IKE_ID_FQDN);
        assert_eq!(&idr.body[4..], b"ims");
        assert!(!idr.body.windows(4).any(|window| window == b"epdg"));
    }

    #[test]
    fn derives_session_keys_into_redacted_snapshot_summary() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "session_keys_ready");
        assert_eq!(
            snapshot.key_schedule.as_ref().map(|plan| plan.prf),
            Some("hmac_sha256")
        );
        assert_eq!(
            snapshot
                .key_schedule
                .as_ref()
                .map(|plan| plan.exported_secret_values),
            Some(false)
        );
        assert!(snapshot
            .transcript
            .iter()
            .any(|event| event.note == "session_keys_ready"));

        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        for forbidden_key in [
            "skeyseed",
            "sk_d",
            "sk_ai",
            "sk_ar",
            "sk_ei",
            "sk_er",
            "sk_pi",
            "sk_pr",
            "initiator_nonce",
            "responder_nonce",
            "shared_secret",
            "plaintext",
            "ciphertext",
            "integrity_tag",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "snapshot must not contain a {forbidden_key} field"
            );
        }
    }

    #[test]
    fn tracks_eap_aka_challenge_to_child_sa_ready_without_aka_values() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_request()
            .expect("build auth request");

        let challenge = [
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0,
            0,
        ];
        machine
            .accept_eap_aka_challenge(&challenge)
            .expect("accept challenge");
        assert_eq!(machine.snapshot().phase, "usim_aka_response_pending");
        assert_eq!(machine.snapshot().next_message_id, 2);
        assert_eq!(
            machine.snapshot().eap.as_ref().map(|eap| eap.subtype),
            Some("challenge")
        );

        machine
            .mark_usim_aka_response_ready()
            .expect("mark response ready");
        machine
            .accept_auth_success(&[3, 7, 0, 4])
            .expect("accept success");
        machine.mark_child_sa_ready().expect("child sa ready");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "child_sa_ready");
        assert!(snapshot
            .transcript
            .iter()
            .any(|event| event.note == "child_sa_ready"));
    }

    #[test]
    fn accepts_encrypted_eap_aka_challenge_without_serializing_attribute_values() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet()
            .expect("build auth request packet");

        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let auth_request = machine
            .private
            .encrypted_payload
            .as_ref()
            .expect("encrypted payload plan");
        assert_eq!(auth_request.first_inner_payload, "identification_initiator");
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");

        machine
            .accept_encrypted_eap_aka_challenge(&encoded)
            .expect("accept encrypted challenge");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "usim_aka_response_pending");
        assert_eq!(snapshot.next_message_id, 2);
        assert_eq!(
            snapshot.eap.as_ref().map(|eap| eap.subtype),
            Some("challenge")
        );
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(!json.to_ascii_lowercase().contains("aabb"));
        for forbidden_key in ["sk_ai", "sk_er", "ciphertext", "integrity_tag"] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn accepts_encrypted_eap_success_after_response_without_payload_values() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");

        let success = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![3, 7, 0, 4],
            }],
        )
        .expect("encrypt success");
        let message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            success,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode success");

        machine
            .accept_encrypted_auth_success(&encoded)
            .expect("accept success");

        assert_eq!(machine.snapshot().phase, "auth_success_accepted");
        assert_eq!(
            machine.snapshot().eap.as_ref().map(|eap| eap.code),
            Some("success")
        );
    }

    #[test]
    fn accepts_eap_aka_notification_round_before_success() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let challenge_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let challenge_encoded = encode_encrypted_message(
            &challenge_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&challenge_encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");

        let notification = vec![
            1,
            8,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            12,
            0,
            0,
            12,
            1,
            0x80,
            0x00,
        ];
        let encrypted_notification = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: notification.clone(),
            }],
        )
        .expect("encrypt notification");
        let notification_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            encrypted_notification,
        );
        let notification_encoded = encode_encrypted_message(
            &notification_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode notification");

        let progress = machine
            .accept_encrypted_auth_progress(&notification_encoded)
            .expect("accept notification");
        assert!(matches!(
            progress,
            IkeAuthProgress::EapAkaNotification { .. }
        ));
        assert_eq!(machine.snapshot().phase, "eap_aka_notification_received");

        machine
            .build_encrypted_eap_response_packet(&[
                2,
                8,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                12,
                0,
                0,
            ])
            .expect("notification response");

        let success = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![3, 8, 0, 4],
            }],
        )
        .expect("encrypt success");
        let success_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            3,
            success,
        );
        let success_encoded = encode_encrypted_message(
            &success_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode success");
        let progress = machine
            .accept_encrypted_auth_progress(&success_encoded)
            .expect("accept success");

        assert_eq!(
            progress,
            IkeAuthProgress::EapSuccess {
                child_sa_included: false
            }
        );
        assert_eq!(machine.snapshot().phase, "auth_success_accepted");
        let json = serde_json::to_string(&machine.snapshot()).expect("serialize snapshot");
        assert!(!json.to_ascii_lowercase().contains("aabb"));
        assert!(!json.to_ascii_lowercase().contains("key_material"));
    }

    #[test]
    fn accepts_eap_aka_identity_round_before_success() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let challenge_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let challenge_encoded = encode_encrypted_message(
            &challenge_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&challenge_encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");

        let identity_request = vec![
            1,
            8,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            5,
            0,
            0,
            10,
            1,
            0,
            0,
        ];
        let encrypted_identity = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: identity_request,
            }],
        )
        .expect("encrypt identity request");
        let identity_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            encrypted_identity,
        );
        let identity_encoded = encode_encrypted_message(
            &identity_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode identity request");

        let progress = machine
            .accept_encrypted_auth_progress(&identity_encoded)
            .expect("accept identity request");
        assert!(matches!(progress, IkeAuthProgress::EapAkaIdentity { .. }));
        assert_eq!(machine.snapshot().phase, "eap_aka_identity_received");

        machine
            .build_encrypted_eap_response_packet(&[
                2,
                8,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                5,
                0,
                0,
            ])
            .expect("identity response");
        assert_eq!(machine.snapshot().phase, "eap_aka_response_ready");
    }

    #[test]
    fn accepts_child_sa_in_same_encrypted_eap_success_response() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let challenge_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let challenge_encoded = encode_encrypted_message(
            &challenge_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&challenge_encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");

        let sa = build_sa_payload(&[child_sa_proposal_from_profile_string(
            "aes128-sha256",
            1,
            &[0x44, 0x55, 0x66, 0x77],
        )
        .expect("child proposal")])
        .expect("sa");
        let mut cfg = vec![2, 0, 0, 0];
        cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP4_ADDRESS.to_be_bytes());
        cfg.extend_from_slice(&4u16.to_be_bytes());
        cfg.extend_from_slice(&[10, 0, 0, 2]);
        cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP4_PCSCF.to_be_bytes());
        cfg.extend_from_slice(&4u16.to_be_bytes());
        cfg.extend_from_slice(&[10, 0, 0, 3]);
        let success_payloads = vec![
            IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![3, 7, 0, 4],
            },
            sa,
            IkePayload {
                payload_type: IkePayloadType::Configuration,
                critical: false,
                body: cfg,
            },
            build_traffic_selector_initiator_payload("ipv4"),
            build_traffic_selector_responder_payload("ipv4"),
        ];
        let success = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &success_payloads,
        )
        .expect("encrypt success");
        let success_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            success,
        );
        let success_encoded = encode_encrypted_message(
            &success_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode success");

        let progress = machine
            .accept_encrypted_auth_progress(&success_encoded)
            .expect("accept success with child sa");

        assert_eq!(
            progress,
            IkeAuthProgress::EapSuccess {
                child_sa_included: true
            }
        );
        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "child_sa_ready");
        assert!(snapshot.child_sa.expect("child").child_sa_present);
        assert!(machine.child_sa_material().is_some());
    }

    #[test]
    fn accepts_child_sa_response_without_responder_auth_when_sa_cp_ts_are_present() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");
        let success = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![3, 7, 0, 4],
            }],
        )
        .expect("encrypt success");
        let success_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            success,
        );
        let success_encoded = encode_encrypted_message(
            &success_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode success");
        machine
            .accept_encrypted_auth_success(&success_encoded)
            .expect("accept success");

        let sa = build_sa_payload(&[child_sa_proposal_from_profile_string(
            "aes128-sha256",
            1,
            &[0x44, 0x55, 0x66, 0x77],
        )
        .expect("child proposal")])
        .expect("sa");
        let mut cfg = vec![2, 0, 0, 0];
        cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP4_ADDRESS.to_be_bytes());
        cfg.extend_from_slice(&4u16.to_be_bytes());
        cfg.extend_from_slice(&[10, 0, 0, 2]);
        cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP4_PCSCF.to_be_bytes());
        cfg.extend_from_slice(&4u16.to_be_bytes());
        cfg.extend_from_slice(&[10, 0, 0, 3]);
        let child_sa_payloads = vec![
            sa,
            IkePayload {
                payload_type: IkePayloadType::Configuration,
                critical: false,
                body: cfg,
            },
            build_traffic_selector_initiator_payload("ipv4"),
            build_traffic_selector_responder_payload("ipv4"),
        ];
        let child_sa = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &child_sa_payloads,
        )
        .expect("encrypt child sa");
        let child_sa_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            3,
            child_sa,
        );
        let child_sa_encoded = encode_encrypted_message(
            &child_sa_message,
            IkePayloadType::SecurityAssociation,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode child sa");

        machine
            .accept_encrypted_child_sa_response(&child_sa_encoded)
            .expect("accept child sa");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "child_sa_ready");
        let child_sa = snapshot.child_sa.as_ref().expect("child summary");
        assert!(!child_sa.responder_auth_present);
        assert!(child_sa.child_sa_present);
        assert!(child_sa.assigned_inner_address_present);
        assert!(child_sa.pcscf_present);
        assert!(child_sa.traffic_selectors_present);
        assert!(machine.child_sa_material().is_some());
    }

    #[test]
    fn parses_child_sa_ipv6_configuration_material_without_serializing_addresses() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        machine
            .accept_sa_init_response(&sa_init_response(&request))
            .expect("accept response");
        machine
            .derive_session_keys(&[0x77; 256])
            .expect("derive session keys");
        machine
            .build_auth_eap_start_packet_for_identity(
                "0234331234567890@nai.epc.mnc033.mcc234.3gppnetwork.org",
            )
            .expect("build auth request packet");
        let bundle = machine
            .private
            .secret_bundle
            .as_ref()
            .expect("secret bundle")
            .clone();
        let challenge = vec![
            1,
            7,
            0,
            12,
            super::super::ike_eap::EAP_TYPE_AKA,
            1,
            0,
            0,
            1,
            1,
            0xaa,
            0xbb,
        ];
        let encrypted = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &[IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: challenge,
            }],
        )
        .expect("encrypt challenge");
        let message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            1,
            encrypted,
        );
        let encoded = encode_encrypted_message(
            &message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode challenge");
        machine
            .accept_encrypted_eap_aka_challenge(&encoded)
            .expect("accept challenge");
        machine
            .build_encrypted_eap_response_packet(&[
                2,
                7,
                0,
                8,
                super::super::ike_eap::EAP_TYPE_AKA,
                1,
                0,
                0,
            ])
            .expect("response");

        let sa = build_sa_payload(&[child_sa_proposal_from_profile_string(
            "aes128-sha256",
            1,
            &[0x44, 0x55, 0x66, 0x77],
        )
        .expect("child proposal")])
        .expect("sa");
        let mut cfg = vec![2, 0, 0, 0];
        cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP6_ADDRESS.to_be_bytes());
        cfg.extend_from_slice(&17u16.to_be_bytes());
        cfg.extend_from_slice(&[
            0x2a, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x10, 64,
        ]);
        for last in [0x20, 0x21] {
            cfg.extend_from_slice(&CFG_ATTR_INTERNAL_IP6_PCSCF.to_be_bytes());
            cfg.extend_from_slice(&16u16.to_be_bytes());
            cfg.extend_from_slice(&[
                0x2a, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, last,
            ]);
        }
        let success_payloads = vec![
            IkePayload {
                payload_type: IkePayloadType::ExtensibleAuthentication,
                critical: false,
                body: vec![3, 7, 0, 4],
            },
            sa,
            IkePayload {
                payload_type: IkePayloadType::Configuration,
                critical: false,
                body: cfg,
            },
            build_traffic_selector_initiator_payload("ipv6"),
            build_traffic_selector_responder_payload("ipv6"),
        ];
        let success = build_encrypted_payload(
            &bundle,
            IkeSkDirection::ResponderToInitiator,
            &success_payloads,
        )
        .expect("encrypt success");
        let success_message = encrypted_message_from_payload(
            machine.private.initiator_spi,
            machine.private.responder_spi.expect("responder spi"),
            IkeExchangeType::IkeAuth,
            false,
            2,
            success,
        );
        let success_encoded = encode_encrypted_message(
            &success_message,
            IkePayloadType::ExtensibleAuthentication,
            &bundle,
            IkeSkDirection::ResponderToInitiator,
        )
        .expect("encode success");

        machine
            .accept_encrypted_auth_progress(&success_encoded)
            .expect("accept success");
        let snapshot = machine.snapshot();
        let child_sa = snapshot.child_sa.as_ref().expect("child summary");

        assert_eq!(child_sa.assigned_inner_address_count, 1);
        assert!(child_sa.assigned_ipv6_prefix_length_present);
        assert_eq!(child_sa.pcscf_count, 2);
        let material = machine.child_sa_material().expect("child material");
        let configuration = material.configuration.as_ref().expect("configuration");
        assert_eq!(configuration.assigned_ipv6_prefix_length, Some(64));
        assert_eq!(configuration.pcscf_addresses.len(), 2);
        assert_eq!(material.secrets.summary().total_secret_bytes, 96);

        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        for forbidden in [
            "2a02",
            "44556677",
            "\"spi\"",
            "\"key_material\"",
            "\"outbound_encryption\"",
            "\"inbound_integrity\"",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn rejects_out_of_order_transition_and_malformed_response() {
        let mut machine = machine(&GB_EE_23433);
        assert!(matches!(
            machine.build_auth_eap_start_request().unwrap_err(),
            IkeStateError::InvalidPhase { .. }
        ));

        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        let mut bad_response = sa_init_response(&request);
        bad_response[0] ^= 0x01;
        assert!(matches!(
            machine.accept_sa_init_response(&bad_response).unwrap_err(),
            IkeStateError::InvalidResponse { .. }
        ));
        assert_eq!(machine.snapshot().phase, "failed");
    }

    #[test]
    fn rejects_sa_init_response_with_unoffered_transform_selection() {
        let mut machine = machine(&GB_EE_23433);
        let request = machine
            .build_sa_init_request()
            .expect("build sa init request");
        let incompatible = build_sa_payload(&[ike_proposal_from_profile_string(
            "aes256-sha256-prfsha512-modp2048",
            1,
        )
        .expect("parse proposal")])
        .expect("build incompatible sa");

        let error = machine
            .accept_sa_init_response(&sa_init_response_with_sa_body(&request, incompatible.body))
            .unwrap_err();

        assert_eq!(
            error,
            IkeStateError::InvalidResponse {
                reason: "selected_sa_not_offered_by_profile"
            }
        );
        assert_eq!(machine.snapshot().phase, "failed");
        let json = serde_json::to_string(&machine.snapshot()).expect("serialize snapshot");
        assert!(!json.to_ascii_lowercase().contains("aabb"));
        assert!(!json.to_ascii_lowercase().contains("key_material"));
    }

    fn informational_message(payload: IkePayload, message_id: u32) -> Vec<u8> {
        IkeMessage::new_request(
            0x1111_2222_3333_4444,
            IkeExchangeType::Informational,
            message_id,
            vec![payload],
        )
        .with_responder_spi(0x9999_aaaa_bbbb_cccc)
        .encode()
        .expect("encode informational")
    }

    #[test]
    fn handles_rekey_notify_without_serializing_spi_or_data_values() {
        let mut machine = machine(&GB_EE_23433);
        let notify = build_notify_payload(
            NotifyProtocolId::Esp,
            &[0xaa, 0xbb, 0xcc, 0xdd],
            16_393,
            &[0xde, 0xad],
        )
        .expect("notify");

        machine
            .handle_control_message(&informational_message(notify, 41))
            .expect("handle notify");

        let snapshot = machine.snapshot();
        assert_eq!(snapshot.phase, "rekey_pending");
        assert_eq!(
            snapshot
                .last_control_event
                .as_ref()
                .map(|event| event.action),
            Some("schedule_rekey")
        );
        assert!(snapshot
            .transcript
            .iter()
            .any(|event| event.note == "schedule_rekey"));

        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        assert!(!json.to_ascii_lowercase().contains("aabbccdd"));
        assert!(!json.to_ascii_lowercase().contains("dead"));
        assert!(!json.to_ascii_lowercase().contains("key_material"));
    }

    #[test]
    fn handles_reauth_delete_and_child_sa_missing_control_events() {
        let mut reauth_machine = machine(&GB_EE_23433);
        let reauth = build_notify_payload(NotifyProtocolId::Ike, &[], 16_403, &[]).expect("notify");
        reauth_machine
            .handle_control_message(&informational_message(reauth, 42))
            .expect("handle reauth");
        assert_eq!(reauth_machine.snapshot().phase, "reauth_required");

        let mut child_machine = machine(&GB_EE_23433);
        let child_missing =
            build_notify_payload(NotifyProtocolId::Esp, &[0xaa, 0xbb, 0xcc, 0xdd], 44, &[])
                .expect("notify");
        child_machine
            .handle_control_message(&informational_message(child_missing, 43))
            .expect("handle child missing");
        assert_eq!(child_machine.snapshot().phase, "child_sa_missing");

        let mut delete_machine = machine(&GB_EE_23433);
        let delete = IkePayload {
            payload_type: IkePayloadType::Delete,
            critical: false,
            body: vec![3, 4, 0, 1, 0xaa, 0xbb, 0xcc, 0xdd],
        };
        delete_machine
            .handle_control_message(&informational_message(delete, 44))
            .expect("handle delete");
        assert_eq!(delete_machine.snapshot().phase, "teardown_requested");
    }

    #[test]
    fn fatal_notify_moves_state_machine_to_failed() {
        let mut machine = machine(&GB_EE_23433);
        let notify = build_notify_payload(NotifyProtocolId::None, &[], 24, &[]).expect("notify");

        let error = machine
            .handle_control_message(&informational_message(notify, 45))
            .unwrap_err();

        assert_eq!(
            error,
            IkeStateError::InvalidResponse {
                reason: "control_event_failed"
            }
        );
        assert_eq!(machine.snapshot().phase, "failed");
        assert!(machine.snapshot().last_error.is_some());
    }
}
