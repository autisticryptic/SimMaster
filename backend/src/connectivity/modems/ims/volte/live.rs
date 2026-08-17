//! Live VoLTE IMS registration driver for the Qualcomm target.
//!
//! This layer wires the pure stage-B pieces together: ModemManager owns the
//! dedicated `ims` bearer, Linux owns IP routing/xfrm, the USIM owns AKA, and
//! the shared `ims::register` driver owns the SIP transaction sequence.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, RwLock as StdRwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use tokio::{process::Command, sync::Mutex};

use crate::{
    connectivity::core::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
        ims_video::{negotiate_video, parse_video_sdp, VideoMediaDescription},
        media::{
            ActiveRtpRelay, MediaRelayMetrics, MediaRelayPolicy, PayloadTypeMapping,
            PendingRtpRelay,
        },
        register::{
            run_register, run_register_observed, run_unregister, RegisterAuthenticator,
            RegisterFailure,
        },
        register_response::RegisterArtifacts,
        registration::{
            ImsRegistrationAccess, RegisteredImsContext, RegistrationLossReason,
            RegistrationRefreshResult, UnregisterResult,
        },
        sip_message::SipHeader,
        supplementary::{
            build_dialog_refer, build_mwi_subscribe, classify_mwi_frame, parse_refer_notify,
            DialogReferRequest, DialogTransfer, MwiIncomingFrame, SubscribeIds,
        },
        voice::{parse_audio_sdp, SdpAddrType, SdpAudioDescription},
        ImsError,
    },
    connectivity::modems::ims::vowifi::{
        carrier_catalog::CatalogAccessKind, profile_store::ProfileStore, profiles::CarrierProfile,
    },
    hardware::cellular::modem_manager::ModemBinding,
    platform::config::{TrunkIncomingMode, TrunkIpConnectMode, VolteIpFamily},
    platform::db::{Database, SmsMessage},
    services::trunk::{
        bridge::{
            parse_rtp_telephone_event, DtmfCapabilities, DtmfSource, MediaOffer, OperatorCommand,
            OperatorEvent, RtpTelephoneEvent, VideoOffer,
        },
        operator::OperatorLink,
    },
    services::{
        notify::notification::NotificationSender,
        supplementary::{
            ut::{XcapAccessContext, XcapDigestProvider},
            SupplementaryRuntime,
        },
    },
};

use crate::connectivity::modems::ims::{
    effective_profile::{
        resolve_effective_device_identity, resolve_effective_ims_profile, EffectiveDeviceIdentity,
        EffectiveImsProfile,
    },
    profile_override::SimOverride,
};

use super::{
    bearer::{
        configure_bearer_network, disconnect_bearer, ensure_ims_bearer_observed, route_pcscf,
        teardown_bearer_network, BearerAttempt, BearerConnection, BearerRequest,
    },
    channel::VolteSipChannel,
    data_slot::DataSlotMode,
    digest_aka,
    errors::{code, VolteError},
    identity,
    ipsec::{self, SecAgree, XfrmInstallPlan},
    native_bearer::{self, NativeImsBearer},
    pcscf::{
        discover_pcscf, discover_pcscf_via_active_at_context, pcscf_socket,
        prefetch_pcscf_from_ims_profile, prepare_ims_profile_context, set_pcscf_reporting,
        ImsProfileLease,
    },
    plan::{FailureClass, ImsConnectionPlan},
    readiness,
    runtime::{RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteStage},
    sip::{self, ImsIdentity, RequestIds},
    sms::{MtIngest, MtReassembler, TRANSPORT_TAG},
};

const QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const MM_MODEM_WAIT_ATTEMPTS: usize = 10;
const MM_MODEM_WAIT_DELAY: Duration = Duration::from_secs(2);
const FAILED_BEARER_MIN_RETENTION: Duration = Duration::from_secs(3);
const MWI_SUBSCRIBE_EXPIRES_SECONDS: u32 = 3600;
const REINVITE_TIMEOUT: Duration = Duration::from_secs(32);
const REFER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(32);
/// Keep two independent IMS dialogs for call waiting. A further call is
/// rejected with a stable busy error before allocating RTP relays.
const MAX_CONCURRENT_CALLS: usize = 2;

fn native_ims_bearer_required(data_slot_mode: DataSlotMode) -> bool {
    !data_slot_mode.ims_on_primary()
}

fn active_ims_profile_prefetch_required(data_slot_mode: DataSlotMode) -> bool {
    native_ims_bearer_required(data_slot_mode)
}

/// Bind the prepared IMS profile when the modem accepts it so PCO and the
/// active AT context refer to the same CID. Some modem firmware
/// intermittently rejects an otherwise valid activation when `profile-id` is
/// present, so retain APN-only as a compatibility fallback.
fn modemmanager_bearer_requests(request: &BearerRequest) -> Vec<BearerRequest> {
    let Some(_) = request.profile_id else {
        return vec![request.clone()];
    };

    let mut apn_only = request.clone();
    apn_only.profile_id = None;
    vec![request.clone(), apn_only]
}

fn may_retry_modemmanager_profile_binding(error: &VolteError) -> bool {
    error.code() == code::RUNTIME_MM_BEARER_CONNECT_FAILED
        && !FailureClass::from_details(error.detail().unwrap_or("")).is_unsafe_to_retry()
}

/// Device-specific inputs formerly hard-coded to modem 0, `/dev/wwan0qmi0`
/// and UIM slot 1. A distinct value is injected for every discovered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolteDeviceBinding {
    pub line_id: String,
    pub modem_id: String,
    pub qmi_device: String,
    pub uim_slot: u8,
    pub equipment_identifier: String,
}

impl VolteDeviceBinding {
    pub fn from_modem(binding: &ModemBinding) -> Result<Self, VolteError> {
        let qmi_device = binding
            .qmi_device
            .clone()
            .ok_or_else(|| VolteError::new("volte_qmi_device_missing"))?;
        Ok(Self {
            line_id: binding.line_id.clone(),
            modem_id: binding.modem_id.clone(),
            qmi_device,
            uim_slot: binding.uim_slot,
            equipment_identifier: binding.equipment_identifier.clone(),
        })
    }
}

/// One independently owned protected SIP session/listener pair. The handle is
/// cloneable so its receive task and API callers coordinate only within the
/// same physical modem/SIM line.
#[derive(Clone)]
pub struct VolteLiveHandle {
    session: Arc<Mutex<Option<VolteLiveSession>>>,
    failed_bearer: Arc<Mutex<Option<RetainedFailedBearer>>>,
    listener: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    operator: OperatorLink,
    supplementary: Arc<StdRwLock<Option<Arc<SupplementaryRuntime>>>>,
}

impl Default for VolteLiveHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl VolteLiveHandle {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            failed_bearer: Arc::new(Mutex::new(None)),
            listener: Arc::new(Mutex::new(None)),
            operator: OperatorLink::default(),
            supplementary: Arc::new(StdRwLock::new(None)),
        }
    }

    pub fn bind_supplementary(&self, runtime: Arc<SupplementaryRuntime>) {
        *self
            .supplementary
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime);
    }

    fn supplementary_runtime(&self) -> Option<Arc<SupplementaryRuntime>> {
        self.supplementary
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn operator_link(&self) -> OperatorLink {
        self.operator.clone()
    }

    pub async fn live_xcap_access(&self) -> Option<XcapAccessContext> {
        let session = self.session.lock().await;
        let session = session.as_ref()?;
        Some(XcapAccessContext {
            access: ImsRegistrationAccess::Volte,
            profile: session.profile,
            local_address: session.channel.route().local_addr.ip(),
            digest: Arc::new(VolteXcapDigestProvider {
                device: session.device.clone(),
                aid: session.aka_aid.clone(),
                username: session.identity.private_user.clone(),
            }),
        })
    }
}

struct VolteXcapDigestProvider {
    device: VolteDeviceBinding,
    aid: Vec<u8>,
    username: String,
}

impl XcapDigestProvider for VolteXcapDigestProvider {
    fn authorize<'a>(
        &'a self,
        challenge: &'a str,
        proxy: bool,
        method: &'a str,
        uri: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<String, crate::connectivity::core::ut::UtError>>
    {
        Box::pin(async move {
            build_volte_xcap_authorization(
                self.device.clone(),
                self.aid.clone(),
                &self.username,
                challenge,
                proxy,
                method,
                uri,
            )
            .await
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn build_volte_xcap_authorization(
    device: VolteDeviceBinding,
    aid: Vec<u8>,
    username: &str,
    challenge_value: &str,
    proxy: bool,
    method: &str,
    uri: &str,
) -> Result<String, crate::connectivity::core::ut::UtError> {
    use crate::connectivity::core::ut::UtError;

    let challenge = digest_aka::parse_digest_challenge(challenge_value, proxy)
        .map_err(|_| UtError::new("ut_xcap_challenge_invalid"))?;
    let aka_challenge = digest_aka::decode_aka_nonce(&challenge.nonce)
        .map_err(|_| UtError::new("ut_xcap_challenge_invalid"))?;
    let aka = tokio::task::spawn_blocking(move || {
        identity::run_usim_aka(
            QMI_PROXY_SOCKET,
            &device.qmi_device,
            device.uim_slot,
            &aid,
            &aka_challenge.rand,
            &aka_challenge.autn,
            2,
            Duration::from_secs(5),
            Duration::from_millis(300),
        )
    })
    .await
    .map_err(|_| UtError::new("ut_xcap_aka_failed"))?
    .map_err(|_| UtError::new("ut_xcap_aka_failed"))?;
    let cnonce = sip::hex_token(8);
    if let Some(auts) = aka.auts.as_deref() {
        return Ok(
            crate::connectivity::core::digest_aka::build_resync_authorization_header_with_digest(
                &challenge,
                username,
                uri,
                auts,
                challenge.qop.as_ref().map(|_| cnonce.as_str()),
                challenge.qop.as_ref().map(|_| "00000001"),
            ),
        );
    }
    let response = digest_aka::compute_aka_response(
        username,
        &challenge.realm,
        &aka,
        &challenge.algorithm,
        method,
        uri,
        &challenge.nonce,
        challenge.qop.as_deref(),
        &cnonce,
        "00000001",
    )
    .map_err(|_| UtError::new("ut_xcap_aka_failed"))?;
    Ok(digest_aka::build_authorization_header(
        &challenge, username, uri, &response, &cnonce, "00000001",
    ))
}

struct VolteLiveSession {
    channel: VolteSipChannel,
    identity: ImsIdentity,
    registration: RegisteredImsContext,
    bearer: BearerConnection,
    /// Set when the bearer was established directly over QMI instead of through
    /// ModemManager. Owns the WDS client/handle, so teardown must release it here;
    /// `mmcli --disconnect` has no object to act on for such a bearer.
    native_bearer: Option<NativeImsBearer>,
    data_slot_mode: DataSlotMode,
    /// Qualcomm P-CSCF reporting is scoped to this session and restored during
    /// teardown. It changes PCO contents only; activation remains owned by WDS.
    pcscf_reporting_cid: Option<u8>,
    /// beta2-style AT IMS context retained until the WDS/SIP session ends.
    ims_profile_lease: Option<ImsProfileLease>,
    pcscf: SocketAddr,
    ip_family: &'static str,
    xfrm_plan: Option<XfrmInstallPlan>,
    register_ids: RequestIds,
    next_register_cseq: u32,
    sip_instance: String,
    security_binding: SecAgree,
    register_variant: VolteRegisterVariant,
    device: VolteDeviceBinding,
    aka_aid: Vec<u8>,
    profile: &'static CarrierProfile,
    /// Owned access-specific values fixed when this session started. Refresh,
    /// SMS and voice must not re-read SimOverrideStore mid-session.
    effective_ims: EffectiveImsProfile,
    voice_calls: HashMap<String, LiveVoiceCall>,
    mwi_subscription: Option<MwiSubscription>,
}

struct MwiSubscription {
    ids: SubscribeIds,
    refresh_at: tokio::time::Instant,
    authenticated: bool,
}

struct RetainedFailedBearer {
    bearer: BearerConnection,
    native_bearer: Option<NativeImsBearer>,
    modem_id: String,
    pcscf_reporting_cid: Option<u8>,
    ims_profile_lease: Option<ImsProfileLease>,
    retained_at: tokio::time::Instant,
}

struct LiveVoiceCall {
    direction: LiveVoiceDirection,
    dialog: sip::DialogIds,
    callee_uri: String,
    invite_branch: String,
    initial_invite: Option<Vec<u8>>,
    internal_offer: MediaOffer,
    operator_local: SocketAddr,
    internal_local: SocketAddr,
    pending_relay: Option<PendingRtpRelay>,
    active_relay: Option<ActiveRtpRelay>,
    ip_answer_wait_armed: bool,
    operator_answered: bool,
    next_cseq: u32,
    media_metrics: Option<Arc<dyn MediaRelayMetrics>>,
    pending_operator_reinvite: Option<Vec<u8>>,
    pending_asterisk_reinvite: bool,
    pending_video_relay: Option<PendingRtpRelay>,
    active_video_relay: Option<ActiveRtpRelay>,
    operator_video_local: Option<SocketAddr>,
    internal_video_local: Option<SocketAddr>,
    pending_media_rollback: Option<LiveVoiceMediaSnapshot>,
    renegotiation_deadline: Option<Instant>,
    /// SDP answer already negotiated from a provisional (18x) response. Per RFC 3262
    /// the operator does not repeat the answer in the final 200 OK, so it is
    /// retained here and reused instead of failing on an empty final body.
    early_answer: Option<String>,
    transfer: Option<DialogTransfer>,
    transfer_deadline: Option<Instant>,
}

/// Confirmed media state retained while a SIP re-INVITE is in flight. A
/// rejected re-INVITE must not replace the live audio relay with its pending
/// video/audio sockets.
#[derive(Clone)]
struct LiveVoiceMediaSnapshot {
    internal_offer: MediaOffer,
    operator_local: SocketAddr,
    internal_local: SocketAddr,
    operator_video_local: Option<SocketAddr>,
    internal_video_local: Option<SocketAddr>,
}

impl LiveVoiceCall {
    fn stage_media_update(
        &mut self,
        offer: MediaOffer,
        pending_relay: PendingRtpRelay,
        operator_local: SocketAddr,
        internal_local: SocketAddr,
        pending_video_relay: Option<PendingRtpRelay>,
        operator_video_local: Option<SocketAddr>,
        internal_video_local: Option<SocketAddr>,
    ) {
        self.pending_media_rollback = Some(LiveVoiceMediaSnapshot {
            internal_offer: self.internal_offer.clone(),
            operator_local: self.operator_local,
            internal_local: self.internal_local,
            operator_video_local: self.operator_video_local,
            internal_video_local: self.internal_video_local,
        });
        self.internal_offer = offer;
        self.pending_relay = Some(pending_relay);
        self.operator_local = operator_local;
        self.internal_local = internal_local;
        self.pending_video_relay = pending_video_relay;
        self.operator_video_local = operator_video_local;
        self.internal_video_local = internal_video_local;
    }

    fn commit_media_update(&mut self) {
        self.pending_media_rollback = None;
    }

    fn rollback_media_update(&mut self) {
        self.pending_relay = None;
        self.pending_video_relay = None;
        let Some(snapshot) = self.pending_media_rollback.take() else {
            return;
        };
        self.internal_offer = snapshot.internal_offer;
        self.operator_local = snapshot.operator_local;
        self.internal_local = snapshot.internal_local;
        self.operator_video_local = snapshot.operator_video_local;
        self.internal_video_local = snapshot.internal_video_local;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveVoiceDirection {
    MobileOriginated,
    MobileTerminated,
}

struct DeviceIdentity {
    ims: ImsIdentity,
    profile: &'static CarrierProfile,
    effective_ims: EffectiveImsProfile,
    effective_device_identity: EffectiveDeviceIdentity,
    aka_aid: Vec<u8>,
    usim_aid: String,
    isim_aid: Option<String>,
    source: &'static str,
}

fn effective_register_target(profile: &EffectiveImsProfile) -> sip::RegisterTarget<'_> {
    sip::RegisterTarget {
        domain: &profile.domain.value,
        realm: &profile.realm.value,
        registrar: profile.registrar.as_ref().map(|field| field.value.as_str()),
    }
}

#[derive(Debug, Clone)]
pub struct VolteSmsSendResult {
    pub message_id: String,
    pub trace_id: String,
    pub part_count: usize,
    pub sip_statuses: Vec<u16>,
}

struct PreparedAuth {
    authorization: String,
    security_client: Option<String>,
    security_verify: Option<String>,
    require_sec_agree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolteInitialAuthorization {
    UriFirstEmptyAka,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolteSecurityClientOffer {
    Full,
    FullSpaced,
    Compact,
}

impl VolteSecurityClientOffer {
    fn build(self, binding: SecAgree, profile: &CarrierProfile) -> String {
        let mechanism = profile
            .ims
            .register
            .security_client_mechanisms
            .first()
            .copied()
            .unwrap_or_default();
        let mut parts = mechanism.split('/');
        let integrity = parts.next().unwrap_or_default();
        let encryption = parts.next().unwrap_or_default();
        let protocol = parts.next().unwrap_or_default();
        let mode = parts.next().unwrap_or_default();
        let separator = if self == Self::FullSpaced { "; " } else { ";" };
        // Field-tested against the Maxis P-CSCF: unquoted `mod=trans` (the
        // form real Android UEs send) is accepted; the quoted RFC-ABNF form
        // is rejected with "400 Bad header field: security-client".
        let mut fields = vec![
            "ipsec-3gpp".to_string(),
            format!("alg={integrity}"),
            format!("ealg={encryption}"),
        ];
        if self != Self::Compact {
            fields.push(format!("prot={protocol}"));
            fields.push(format!("mod={mode}"));
        }
        fields.extend([
            format!("spi-c={}", binding.spi_c),
            format!("spi-s={}", binding.spi_s),
            format!("port-c={}", binding.port_c),
            format!("port-s={}", binding.port_s),
        ]);
        fields.join(separator)
    }
}

impl VolteInitialAuthorization {
    fn label(self) -> &'static str {
        match self {
            Self::UriFirstEmptyAka => "aka_empty_uri_first",
            Self::None => "none",
        }
    }

    fn build(self, realm: &str, identity: &ImsIdentity, request_uri: &str) -> Option<String> {
        match self {
            Self::UriFirstEmptyAka => {
                Some(digest_aka::build_initial_authorization_header_uri_first(
                    &identity.private_user,
                    realm,
                    request_uri,
                ))
            }
            Self::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VolteRegisterVariant {
    label: &'static str,
    authorization: VolteInitialAuthorization,
    policy: sip::RegisterRequestPolicy,
    server_required_sec_agree: bool,
    security_client_offer: VolteSecurityClientOffer,
}

impl VolteRegisterVariant {
    fn requiring_sec_agree(self) -> Self {
        let label = match self.authorization {
            VolteInitialAuthorization::UriFirstEmptyAka => {
                "ims_features_aka_uri_first_sec_agree_required"
            }
            VolteInitialAuthorization::None => {
                "ims_features_no_initial_authorization_sec_agree_required"
            }
        };
        Self {
            label,
            policy: sip::RegisterRequestPolicy {
                advertise_sec_agree: true,
                require_sec_agree: true,
                proxy_require_sec_agree: true,
                ..self.policy
            },
            server_required_sec_agree: true,
            ..self
        }
    }

    fn requiring_sec_agree_without_proxy(self) -> Self {
        let label = match self.authorization {
            VolteInitialAuthorization::UriFirstEmptyAka => {
                "ims_features_aka_uri_first_sec_agree_require_only"
            }
            VolteInitialAuthorization::None => {
                "ims_features_no_initial_authorization_sec_agree_require_only"
            }
        };
        Self {
            label,
            policy: sip::RegisterRequestPolicy {
                require_sec_agree: true,
                proxy_require_sec_agree: false,
                ..self.policy
            },
            ..self
        }
    }

    fn with_compact_security_client(self) -> Self {
        let label = match self.authorization {
            VolteInitialAuthorization::UriFirstEmptyAka => {
                "ims_features_aka_uri_first_sec_agree_compact_security"
            }
            VolteInitialAuthorization::None => {
                "ims_features_no_initial_authorization_sec_agree_compact_security"
            }
        };
        Self {
            label,
            security_client_offer: VolteSecurityClientOffer::Compact,
            ..self
        }
    }

    fn with_spaced_security_client(self) -> Self {
        let label = match self.authorization {
            VolteInitialAuthorization::UriFirstEmptyAka => {
                "ims_features_aka_uri_first_sec_agree_spaced_security"
            }
            VolteInitialAuthorization::None => {
                "ims_features_no_initial_authorization_sec_agree_spaced_security"
            }
        };
        Self {
            label,
            security_client_offer: VolteSecurityClientOffer::FullSpaced,
            ..self
        }
    }
}

#[cfg(test)]
const VOLTE_REGISTER_VARIANTS: &[VolteRegisterVariant] = &[
    // This request shape completed AKA/IPsec registration on the target
    // Qualcomm/Maxis deployment. Keep it first so exploratory carrier variants
    // cannot alter P-CSCF transaction state before the proven form.
    VolteRegisterVariant {
        label: "reference_sms_sec_agree",
        authorization: VolteInitialAuthorization::None,
        policy: sip::RegisterRequestPolicy::LEGACY,
        server_required_sec_agree: false,
        security_client_offer: VolteSecurityClientOffer::Full,
    },
    VolteRegisterVariant {
        label: "ims_features_aka_uri_first",
        authorization: VolteInitialAuthorization::UriFirstEmptyAka,
        policy: sip::RegisterRequestPolicy {
            advertise_sec_agree: true,
            require_sec_agree: false,
            proxy_require_sec_agree: false,
            include_mmtel_features: true,
            include_video_feature: false,
            include_route_header: true,
            include_visited_network: true,
        },
        server_required_sec_agree: false,
        security_client_offer: VolteSecurityClientOffer::Full,
    },
    VolteRegisterVariant {
        label: "ims_features_no_initial_authorization",
        authorization: VolteInitialAuthorization::None,
        policy: sip::RegisterRequestPolicy {
            advertise_sec_agree: true,
            require_sec_agree: false,
            proxy_require_sec_agree: false,
            include_mmtel_features: true,
            include_video_feature: false,
            include_route_header: true,
            include_visited_network: true,
        },
        server_required_sec_agree: false,
        security_client_offer: VolteSecurityClientOffer::Full,
    },
];

fn register_variants(profile: &CarrierProfile) -> Vec<VolteRegisterVariant> {
    let authorization = match profile.ims.register.initial_authorization {
        "aka_empty" | "digest_empty" | "implementation_variant" => {
            VolteInitialAuthorization::UriFirstEmptyAka
        }
        _ => VolteInitialAuthorization::None,
    };
    let disabled = profile.ims.register.sec_agree_mode == "disabled";
    let required = !disabled
        && (profile.ims.register.sec_agree_mode == "required"
            || profile.ims.register.require_sec_agree_headers);
    let advertise = !disabled
        && profile
            .ims
            .register
            .supported_header
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("sec-agree"));
    vec![VolteRegisterVariant {
        label: profile.ims.register.live_header_variant_set,
        authorization,
        policy: sip::RegisterRequestPolicy {
            advertise_sec_agree: advertise,
            require_sec_agree: required,
            proxy_require_sec_agree: profile.ims.register.proxy_require_sec_agree_headers,
            include_mmtel_features: profile.ims.register.include_mmtel_features,
            include_video_feature: false,
            include_route_header: profile.ims.register.include_route_header,
            include_visited_network: profile.ims.register.include_visited_network,
        },
        server_required_sec_agree: required,
        security_client_offer: VolteSecurityClientOffer::Full,
    }]
}

fn security_server_matches_profile(profile: &CarrierProfile, value: &str) -> bool {
    let mut parameters = HashMap::new();
    for part in value.split(';').skip(1) {
        if let Some((name, raw)) = part.split_once('=') {
            parameters.insert(
                name.trim().to_ascii_lowercase(),
                raw.trim().trim_matches('"').to_ascii_lowercase(),
            );
        }
    }
    profile
        .ims
        .register
        .security_client_mechanisms
        .iter()
        .any(|mechanism| {
            let mut expected = mechanism.split('/');
            ["alg", "ealg", "prot", "mod"]
                .into_iter()
                .zip(&mut expected)
                .all(|(name, expected)| {
                    parameters
                        .get(name)
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
                })
        })
}

struct VolteRegisterAuthenticator {
    identity: ImsIdentity,
    ids: RequestIds,
    sip_instance: String,
    offered_security: String,
    offered_security_binding: SecAgree,
    route: ImsRoute,
    pending: Option<PreparedAuth>,
    mode: RegistrationMode,
    xfrm_plan: Option<XfrmInstallPlan>,
    device: VolteDeviceBinding,
    runtime: VolteRuntime,
    reuse_security: bool,
    aka_aid: Vec<u8>,
    register_policy: sip::RegisterRequestPolicy,
    profile: &'static CarrierProfile,
    effective_ims: EffectiveImsProfile,
    expires_seconds: u32,
}

impl VolteRegisterAuthenticator {
    fn new(
        identity: ImsIdentity,
        ids: RequestIds,
        sip_instance: String,
        offered_security_binding: SecAgree,
        offered_security: String,
        route: ImsRoute,
        device: VolteDeviceBinding,
        runtime: VolteRuntime,
        reuse_security: bool,
        aka_aid: Vec<u8>,
        register_policy: sip::RegisterRequestPolicy,
        profile: &'static CarrierProfile,
        effective_ims: EffectiveImsProfile,
    ) -> Self {
        Self {
            identity,
            ids,
            sip_instance,
            offered_security,
            offered_security_binding,
            route,
            pending: None,
            mode: RegistrationMode::None,
            xfrm_plan: None,
            device,
            runtime,
            reuse_security,
            aka_aid,
            register_policy,
            profile,
            effective_ims,
            expires_seconds: profile.ims.register.expires_seconds,
        }
    }

    fn with_expires_seconds(mut self, expires_seconds: u32) -> Self {
        self.expires_seconds = expires_seconds;
        self
    }
}

impl RegisterAuthenticator<VolteSipChannel> for VolteRegisterAuthenticator {
    async fn prepare_authenticated_channel(
        &mut self,
        challenge_response: &[u8],
        channel: &mut VolteSipChannel,
    ) -> Result<(), ImsError> {
        self.runtime
            .update(|state| state.stage = VolteStage::IdentityAka)
            .await;
        let challenge = parse_digest_challenge(challenge_response).map_err(to_ims_error)?;
        let aka_challenge = digest_aka::decode_aka_nonce(&challenge.nonce).map_err(to_ims_error)?;
        let aid = self.aka_aid.clone();
        let rand = aka_challenge.rand;
        let autn = aka_challenge.autn;
        let qmi_device = self.device.qmi_device.clone();
        let uim_slot = self.device.uim_slot;
        let aka = tokio::task::spawn_blocking(move || {
            identity::run_usim_aka(
                QMI_PROXY_SOCKET,
                qmi_device.as_str(),
                uim_slot,
                &aid,
                &rand,
                &autn,
                2,
                Duration::from_secs(5),
                Duration::from_millis(300),
            )
        })
        .await
        .map_err(|_| ImsError::new(code::USIM_AKA_FAILED))?
        .map_err(to_ims_error)?;

        if let Some(auts) = aka.auts.as_deref() {
            self.route = channel.route();
            let request_uri = sip::register_request_uri_with_target(
                self.profile,
                effective_register_target(&self.effective_ims),
                &self.route,
            );
            let security_enabled = self.profile.ims.register.sec_agree_mode != "disabled"
                && !self
                    .profile
                    .ims
                    .register
                    .security_client_mechanisms
                    .is_empty();
            self.pending = Some(PreparedAuth {
                authorization: digest_aka::build_resync_authorization_header(
                    &challenge,
                    &self.identity.private_user,
                    &request_uri,
                    auts,
                ),
                security_client: security_enabled.then(|| self.offered_security.clone()),
                security_verify: None,
                require_sec_agree: security_enabled
                    && self.profile.ims.register.require_sec_agree_headers,
            });
            return Ok(());
        }

        let security_server_values = sip::header_values(challenge_response, "Security-Server");
        let security_server = security_server_values.iter().find_map(|value| {
            if self.profile.ims.register.strict_security_server_offer
                && !security_server_matches_profile(self.profile, value)
            {
                return None;
            }
            ipsec::parse_security_server(&value)
                .ok()
                .map(|sec| (sec, value.clone()))
        });
        let (security_client, security_verify, require_sec_agree) = if self.reuse_security {
            let security_verify = channel.security_verify().map(str::to_string);
            self.mode = if security_verify.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            (None, security_verify.clone(), security_verify.is_some())
        } else if let Some((selected, verify)) = security_server {
            self.runtime
                .update(|state| state.stage = VolteStage::Ipsec)
                .await;
            let route = channel.route();
            let algs = ipsec::xfrm_algs_from_security_server(&verify).map_err(to_ims_error)?;
            let plan = ipsec::build_install_plan_with_algs(
                route.local_addr.ip(),
                route.pcscf_addr.ip(),
                &self.offered_security_binding,
                &selected,
                &aka.ik,
                &aka.ck,
                algs,
            )
            .map_err(to_ims_error)?;
            ipsec::install_plan(&plan).map_err(to_ims_error)?;
            let protected_send_route = ImsRoute {
                local_addr: SocketAddr::new(
                    route.local_addr.ip(),
                    self.offered_security_binding.port_c,
                ),
                pcscf_addr: SocketAddr::new(route.pcscf_addr.ip(), selected.port_s),
                transport: SipTransport::Udp,
            };
            let receive_local =
                SocketAddr::new(route.local_addr.ip(), self.offered_security_binding.port_s);
            let receive_remote = SocketAddr::new(route.pcscf_addr.ip(), selected.port_c);
            if let Err(error) = channel.activate_security(
                protected_send_route,
                receive_local,
                receive_remote,
                Some(verify.clone()),
            ) {
                ipsec::uninstall_plan(&plan);
                return Err(error);
            }
            self.xfrm_plan = Some(plan);
            self.mode = RegistrationMode::Ipsec;
            (Some(self.offered_security.clone()), Some(verify), true)
        } else if self.profile.ims.register.sec_agree_mode == "required"
            || (self.profile.ims.register.strict_security_server_offer
                && !security_server_values.is_empty())
        {
            return Err(ImsError::new(code::SECURITY_SERVER_MISSING));
        } else {
            self.mode = RegistrationMode::Udp;
            (None, None, false)
        };
        self.route = channel.route();
        let request_uri = sip::register_request_uri_with_target(
            self.profile,
            effective_register_target(&self.effective_ims),
            &self.route,
        );
        let cnonce = sip::hex_token(8);
        let nc = "00000001";
        let proof = digest_aka::compute_aka_response(
            &self.identity.private_user,
            &challenge.realm,
            &aka,
            &challenge.algorithm,
            "REGISTER",
            &request_uri,
            &challenge.nonce,
            challenge.qop.as_deref(),
            &cnonce,
            nc,
        )
        .map_err(to_ims_error)?;
        let authorization = digest_aka::build_authorization_header(
            &challenge,
            &self.identity.private_user,
            &request_uri,
            &proof,
            &cnonce,
            nc,
        );
        self.pending = Some(PreparedAuth {
            authorization,
            security_client,
            security_verify,
            require_sec_agree,
        });
        Ok(())
    }

    async fn authenticated_request(
        &mut self,
        _challenge_response: &[u8],
        cseq: u32,
    ) -> Result<Vec<u8>, ImsError> {
        self.runtime
            .update(|state| state.stage = VolteStage::RegisterAuthenticated)
            .await;
        let prepared = self
            .pending
            .take()
            .ok_or(ImsError::new("volte_register_auth_not_prepared"))?;
        let mut ids = self.ids.clone();
        ids.cseq = self.ids.cseq.saturating_add(cseq.saturating_sub(1));
        Ok(sip::build_register_from_profile_with_target(
            self.profile,
            effective_register_target(&self.effective_ims),
            sip::RegisterPhase::Authenticated,
            &self.identity,
            &self.route,
            &ids,
            self.expires_seconds,
            Some(&prepared.authorization),
            prepared.security_client.as_deref(),
            prepared.security_verify.as_deref(),
            &self.sip_instance,
            sip::RegisterRequestPolicy {
                require_sec_agree: prepared.require_sec_agree,
                ..self.register_policy
            },
        ))
    }
}

pub async fn connect_live_for_line(
    live: &VolteLiveHandle,
    device: &VolteDeviceBinding,
    runtime: &Arc<VolteRuntime>,
    voice_enabled: bool,
    line_ip_families: &[VolteIpFamily],
    allow_roaming: bool,
    data_slot_mode: DataSlotMode,
    dedupe_enabled: bool,
    profile_store: ProfileStore,
    sim_override: SimOverride,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
) -> Result<VolteRuntimeStatus, VolteError> {
    // Connection, media and address-family intent are all supplied for this
    // physical line.
    let _advance = runtime.advance_guard().await;
    if live.session.lock().await.is_some() {
        return Ok(runtime.status().await);
    }
    cleanup_retained_failed_bearer(live).await;
    let generation = runtime.generation();
    runtime
        .update(|state| {
            state.phase = VoltePhase::Starting;
            state.stage = VolteStage::Starting;
            state.session_started_at = Some(now());
            state.last_error = None;
            state.qmi_device = Some(device.qmi_device.clone());
            state.bearer_interface = None;
            state.bearer_ip_type = None;
            state.bearer_path = None;
            state.at_cid = None;
            state.current_ip_family = None;
            state.connection_attempts.clear();
        })
        .await;

    let plan = ImsConnectionPlan::from_families(line_ip_families);

    match connect_inner(
        live,
        runtime,
        generation,
        device,
        plan,
        allow_roaming,
        data_slot_mode,
        &profile_store,
        &sim_override,
    )
    .await
    {
        Ok(session) => {
            let mode = if session.xfrm_plan.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            let pcscf = session.pcscf.to_string();
            let data_path_mode = session.data_slot_mode.as_str().to_string();
            *live.session.lock().await = Some(session);
            live.operator.set_ready(voice_enabled);
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Registered;
                    state.stage = VolteStage::Registered;
                    state.registration_mode = mode;
                    state.pcscf = Some(pcscf);
                    state.registered_at = Some(now());
                    state.data_path_mode = Some(data_path_mode);
                    state.recovery_state = super::runtime::VolteRecoveryState::Registered;
                    state.manual_retry_available = false;
                    state.next_retry_at = None;
                })
                .await;
            start_live_listener(
                live.clone(),
                device.line_id.clone(),
                Arc::clone(runtime),
                database,
                notification_sender,
                generation,
                dedupe_enabled,
            )
            .await;
            Ok(runtime.status().await)
        }
        Err(error) => {
            let message = error.to_string();
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Degraded;
                    state.last_error = Some(message);
                    state.last_failure_at = Some(now());
                })
                .await;
            Err(error)
        }
    }
}

async fn connect_inner(
    live: &VolteLiveHandle,
    runtime: &VolteRuntime,
    generation: u64,
    device: &VolteDeviceBinding,
    plan: ImsConnectionPlan,
    allow_roaming: bool,
    data_slot_mode: DataSlotMode,
    profile_store: &ProfileStore,
    sim_override: &SimOverride,
) -> Result<VolteLiveSession, VolteError> {
    // The canonical connection plan is built by the caller from this line's
    // explicit ordered families. All family-selection consumers (AT probe
    // order, bearer fallback, IPv6 preflight hint, SIP local-address order)
    // derive from this one object.
    let mut device = resolve_device_binding(device).await?;

    // beta2 readiness gate: wait for the QMI auto-activate marker to settle before
    // driving the modem, so IMS setup does not race the initial UIM provisioning.
    // This is advisory — a timeout falls through to the ordinary modem-readiness
    // checks rather than failing (matching beta2's
    // "continuing with modem readiness checks").
    tracing::info!("Waiting for initial QMI UIM provisioning to settle");
    let readiness = readiness::wait_for_qmi_ready().await;
    match readiness {
        readiness::ReadinessOutcome::Ready => tracing::info!("{}", readiness.log_message()),
        readiness::ReadinessOutcome::TimedOut => tracing::warn!("{}", readiness.log_message()),
    }

    runtime
        .update(|state| state.stage = VolteStage::Identity)
        .await;
    let device_identity = load_device_identity(&device, profile_store, sim_override).await?;
    let ims_apn = device_identity
        .effective_ims
        .ims_apn
        .as_ref()
        .map(|field| field.value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| VolteError::new(code::CARRIER_IMS_APN_MISSING))?;
    runtime
        .update(|state| {
            state.identity_source = Some(device_identity.source.to_string());
            state.usim_aid = Some(device_identity.usim_aid.clone());
            state.isim_aid = device_identity.isim_aid.clone();
        })
        .await;
    ensure_generation(runtime, generation)?;

    runtime
        .update(|state| state.stage = VolteStage::ImsContext)
        .await;

    // Beta8 only pre-activates the IMS AT profile when IMS itself uses the
    // native DATA6 WDS path. On primary qmi0, ModemManager must remain the sole
    // PDP activation owner; activating the same CID here first makes its bearer
    // connect fail with an internal error.
    ensure_generation(runtime, generation)?;
    device = resolve_device_binding(&device).await?;

    runtime
        .update(|state| state.stage = VolteStage::Bearer)
        .await;
    let mut prefetched_pcscf = Vec::new();
    let mut ims_profile_lease = None;
    let native_required = native_ims_bearer_required(data_slot_mode);
    let ims_profile = if active_ims_profile_prefetch_required(data_slot_mode) {
        match prefetch_pcscf_from_ims_profile(&device.modem_id, &plan, ims_apn).await {
            Ok(prefetch) => {
                let cid = prefetch.lease.cid;
                prefetched_pcscf = prefetch.candidates;
                tracing::info!(
                    cid,
                    pcscf_count = prefetched_pcscf.len(),
                    "Prepared Beta8 native IMS profile and retained its AT context"
                );
                runtime.update(|state| state.at_cid = Some(cid)).await;
                ims_profile_lease = Some(prefetch.lease);
                Some(super::pcscf::ImsProfileContext {
                    cid,
                    created: false,
                })
            }
            Err(prefetch_error) => {
                tracing::warn!(
                    error = %prefetch_error,
                    "Native VoLTE profile P-CSCF prefetch failed; falling back to bearer discovery"
                );
                match prepare_ims_profile_context(&device.modem_id, &plan, ims_apn).await {
                    Ok(profile) => {
                        tracing::info!(
                            cid = profile.cid,
                            created = profile.created,
                            "Selected fallback IMS 3GPP profile"
                        );
                        runtime
                            .update(|state| state.at_cid = Some(profile.cid))
                            .await;
                        Some(profile)
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "Unable to select an IMS 3GPP profile; continuing with APN-only bearer setup"
                        );
                        None
                    }
                }
            }
        }
    } else {
        match prepare_ims_profile_context(&device.modem_id, &plan, ims_apn).await {
            Ok(profile) => {
                tracing::info!(
                    cid = profile.cid,
                    created = profile.created,
                    "Prepared inactive IMS profile for ModemManager activation"
                );
                runtime
                    .update(|state| state.at_cid = Some(profile.cid))
                    .await;
                Some(profile)
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Unable to select an IMS 3GPP profile; continuing with APN-only bearer setup"
                );
                None
            }
        }
    };
    let mut request = BearerRequest::for_apn(ims_apn, allow_roaming);
    request.profile_id = ims_profile.map(|profile| u32::from(profile.cid));

    // The beta2 prefetch lease already armed reporting and activated its
    // context. Only the compatibility fallback still needs the standalone
    // reporting command.
    let pcscf_reporting_cid = if ims_profile_lease.is_some() {
        None
    } else if let Some(profile) = ims_profile {
        match set_pcscf_reporting(&device.modem_id, profile.cid, true).await {
            Ok(()) => {
                tracing::info!(cid = profile.cid, "Enabled IMS P-CSCF reporting");
                Some(profile.cid)
            }
            Err(error) => {
                tracing::warn!(cid = profile.cid, error = %error, "IMS P-CSCF reporting command failed; bearer PCO and DNS discovery remain available");
                None
            }
        }
    } else {
        None
    };

    // When qmi0 already carries ordinary data, establish IMS directly on DATA6.
    // Otherwise the ModemManager path below establishes IMS on primary qmi0.
    let mut native_bearer = None;
    if native_required {
        runtime
            .record_attempt(
                VolteStage::Bearer,
                None,
                "started",
                None,
                Some(format!("native_qmi:{}", device.qmi_device)),
            )
            .await;
        match native_bearer::establish_native_ims_bearer(
            &device.qmi_device,
            &device.modem_id,
            &request,
            &plan,
        )
        .await
        {
            Ok(established) => {
                runtime
                    .record_attempt(
                        VolteStage::Bearer,
                        Some(established.connection.ip_type.as_str()),
                        "succeeded",
                        None,
                        Some(format!(
                            "native_qmi:{}:netdev={}:{}",
                            device.qmi_device, established.interface, established.netdev_method
                        )),
                    )
                    .await;
                native_bearer = Some(established);
            }
            Err(error) => {
                let class = FailureClass::from_details(error.detail().unwrap_or(""));
                runtime
                    .record_attempt(
                        VolteStage::Bearer,
                        None,
                        "failed",
                        Some(&error),
                        Some("native_qmi".to_string()),
                    )
                    .await;
                // A wedged baseband must not be handed to ModemManager for a
                // second activation attempt: that is precisely what escalates a
                // subsystem restart into a dead device.
                if class.is_unsafe_to_retry() || native_required {
                    disable_pcscf_reporting(&device.modem_id, pcscf_reporting_cid).await;
                    cleanup_ims_profile_lease(ims_profile_lease.take()).await;
                    return Err(error);
                }
                tracing::warn!(
                    error = %error,
                    "Native QMI IMS bearer failed; falling back to the ModemManager bearer"
                );
            }
        }
    }

    // A native session already carries its own connection details; otherwise fall
    // back to letting ModemManager create and connect the bearer.
    let mut bearer = if let Some(established) = native_bearer.as_ref() {
        established.connection.clone()
    } else {
        let requests = modemmanager_bearer_requests(&request);
        let mut connected = None;
        let mut last_error = None;
        for (index, candidate) in requests.iter().enumerate() {
            match ensure_bearer_with_runtime(runtime, &device.modem_id, candidate, &plan).await {
                Ok(bearer) => {
                    connected = Some(bearer);
                    break;
                }
                Err(error) => {
                    let has_fallback = index + 1 < requests.len();
                    if has_fallback && may_retry_modemmanager_profile_binding(&error) {
                        tracing::warn!(
                            error = %error,
                            profile_id = ?requests[index + 1].profile_id,
                            "Profile-bound IMS bearer failed; retrying with APN-only compatibility mode"
                        );
                        last_error = Some(error);
                        continue;
                    }
                    last_error = Some(error);
                    break;
                }
            }
        }
        match connected {
            Some(bearer) => bearer,
            None => {
                if let Some(established) = native_bearer.take() {
                    native_bearer::release_native_ims_bearer(established).await;
                }
                disable_pcscf_reporting(&device.modem_id, pcscf_reporting_cid).await;
                cleanup_ims_profile_lease(ims_profile_lease.take()).await;
                return Err(last_error
                    .unwrap_or_else(|| VolteError::new(code::RUNTIME_MM_BEARER_CONNECT_FAILED)));
            }
        }
    };
    for candidate in prefetched_pcscf.drain(..) {
        if !bearer.settings.pcscf.contains(&candidate) {
            bearer.settings.pcscf.push(candidate);
        }
    }
    if !bearer.settings.pcscf.is_empty() {
        tracing::info!(
            pcscf_count = bearer.settings.pcscf.len(),
            "Native VoLTE using P-CSCF candidates prefetched from IMS profile"
        );
    }
    // WDS PCO, the active context, and IMS DNS remain ordered fallbacks when
    // the profile prefetch did not yield an address.
    if bearer.settings.pcscf.is_empty() {
        tracing::info!("VoLTE bearer delivered no P-CSCF via PCO; reading the active IMS context");
        runtime
            .record_attempt(
                VolteStage::Pcscf,
                None,
                "started",
                None,
                Some("at_cgcontrdp_fallback".to_string()),
            )
            .await;
        match discover_pcscf_via_active_at_context(&device.modem_id, &plan, ims_apn).await {
            Ok(discovery) => {
                runtime
                    .record_attempt(
                        VolteStage::Pcscf,
                        None,
                        "succeeded",
                        None,
                        Some(format!("at_cgcontrdp_fallback:cid={}", discovery.cid)),
                    )
                    .await;
                runtime
                    .update(|state| state.at_cid = Some(discovery.cid))
                    .await;
                for candidate in discovery.candidates {
                    if !bearer.settings.pcscf.contains(&candidate) {
                        bearer.settings.pcscf.push(candidate);
                    }
                }
            }
            Err(error) => {
                runtime
                    .record_attempt(
                        VolteStage::Pcscf,
                        None,
                        "failed",
                        Some(&error),
                        Some("at_cgcontrdp_fallback".to_string()),
                    )
                    .await;
                tracing::warn!(
                    error = %error,
                    "VoLTE active-context P-CSCF fallback failed; the SIP loop will still try DNS discovery"
                );
            }
        }
    }
    let result = async {
        runtime
            .update(|state| {
                state.stage = VolteStage::IpConfig;
                state.bearer_interface = Some(bearer.interface.clone());
                state.bearer_ip_type = Some(bearer.ip_type.clone());
                state.bearer_path = Some(bearer.path.clone());
            })
            .await;
        if let Err(error) = configure_bearer_network(&bearer).await {
            runtime
                .record_attempt(
                    VolteStage::IpConfig,
                    None,
                    "failed",
                    Some(&error),
                    Some(bearer.interface.clone()),
                )
                .await;
            return Err(error);
        }
        runtime
            .record_attempt(
                VolteStage::IpConfig,
                None,
                "succeeded",
                None,
                Some(bearer.interface.clone()),
            )
            .await;
        ensure_generation(runtime, generation)?;
        let local_addrs = bearer.settings.ordered_local_addrs(&plan);
        if local_addrs.is_empty() {
            // The bearer connected but carries no address in a family the
            // configured preference admits (e.g. an ipv4-only preference on an
            // IPv6-only bearer). This is a family-support mismatch, not a
            // generic missing-settings error.
            let has_any_addr =
                bearer.settings.ipv4_address.is_some() || bearer.settings.ipv6_address.is_some();
            let code = if has_any_addr {
                code::RUNTIME_IMS_FAMILY_UNSUPPORTED
            } else {
                code::IP_SETTINGS_MISSING
            };
            return Err(VolteError::new(code));
        }
        let mut last_error = None;
        for (index, local_addr) in local_addrs.iter().copied().enumerate() {
            ensure_generation(runtime, generation)?;
            let family = ip_family_name(local_addr);
            runtime
                .update(|state| state.current_ip_family = Some(family.to_string()))
                .await;
            runtime
                .record_attempt(VolteStage::Pcscf, Some(family), "started", None, None)
                .await;
            match connect_family(
                runtime,
                &bearer,
                &device_identity,
                local_addr,
                &device,
                live.operator.video_enabled(),
            )
            .await
            {
                Ok(session) => {
                    runtime
                        .record_attempt(
                            VolteStage::Registered,
                            Some(family),
                            "succeeded",
                            None,
                            None,
                        )
                        .await;
                    return Ok(session);
                }
                Err(error)
                    if index + 1 < local_addrs.len()
                        && FailureClass::from_error(&error).is_retryable_family() =>
                {
                    runtime
                        .record_attempt(
                            VolteStage::Pcscf,
                            Some(family),
                            "failed",
                            Some(&error),
                            Some("trying_next_family".to_string()),
                        )
                        .await;
                    last_error = Some(error);
                }
                Err(error) => {
                    runtime
                        .record_attempt(
                            VolteStage::Pcscf,
                            Some(family),
                            "failed",
                            Some(&error),
                            None,
                        )
                        .await;
                    return Err(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED)))
    }
    .await;
    if let Err(error) = &result {
        if should_retain_failed_bearer(error) {
            tracing::warn!(
                error = %error,
                bearer_path = %bearer.path,
                retention_secs = FAILED_BEARER_MIN_RETENTION.as_secs(),
                "Retaining failed IMS bearer to avoid an immediate firmware disconnect race"
            );
            *live.failed_bearer.lock().await = Some(RetainedFailedBearer {
                bearer: bearer.clone(),
                native_bearer: native_bearer.take(),
                modem_id: device.modem_id.clone(),
                pcscf_reporting_cid,
                ims_profile_lease: ims_profile_lease.take(),
                retained_at: tokio::time::Instant::now(),
            });
            return Err(error.clone());
        }
        teardown_bearer_network(&bearer).await;
        // `disconnect_bearer` is a no-op for a native path; the WDS session behind
        // it is owned by the handle below and released explicitly.
        disconnect_bearer(&bearer.path).await;
        if let Some(established) = native_bearer.take() {
            native_bearer::release_native_ims_bearer(established).await;
        }
        disable_pcscf_reporting(&device.modem_id, pcscf_reporting_cid).await;
        cleanup_ims_profile_lease(ims_profile_lease.take()).await;
    }
    result.map(|mut session| {
        // Hand the session ownership of the native bearer so `cleanup_live_session`
        // can stop the WDS session and release its client id. Without this the PDP
        // context would outlive the SIP session.
        session.native_bearer = native_bearer;
        session.data_slot_mode = data_slot_mode;
        session.pcscf_reporting_cid = pcscf_reporting_cid;
        session.ims_profile_lease = ims_profile_lease;
        session
    })
}

fn bearer_stage(ip_type: &str) -> VolteStage {
    match ip_type {
        "ipv4v6" => VolteStage::BearerDual,
        "ipv4" => VolteStage::BearerIpv4,
        "ipv6" => VolteStage::BearerIpv6,
        _ => VolteStage::Bearer,
    }
}

async fn ensure_bearer_with_runtime(
    runtime: &VolteRuntime,
    modem_id: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
) -> Result<BearerConnection, VolteError> {
    let observed_runtime = runtime.clone();
    ensure_ims_bearer_observed(modem_id, request, plan, move |attempt: BearerAttempt| {
        let runtime = observed_runtime.clone();
        async move {
            let stage = bearer_stage(&attempt.ip_type);
            let family = match attempt.ip_type.as_str() {
                "ipv4v6" => Some("dual"),
                "ipv4" => Some("ipv4"),
                "ipv6" => Some("ipv6"),
                _ => None,
            };
            runtime
                .update(|state| {
                    state.stage = stage;
                    state.bearer_ip_type = Some(attempt.ip_type.clone());
                })
                .await;
            runtime
                .record_attempt(
                    stage,
                    family,
                    &attempt.outcome,
                    attempt.error.as_ref(),
                    Some(attempt.source),
                )
                .await;
        }
    })
    .await
}

async fn connect_family(
    runtime: &VolteRuntime,
    bearer: &BearerConnection,
    device_identity: &DeviceIdentity,
    local_addr: IpAddr,
    device: &VolteDeviceBinding,
    video_capability_enabled: bool,
) -> Result<VolteLiveSession, VolteError> {
    runtime
        .update(|state| state.stage = VolteStage::Pcscf)
        .await;
    let pcscf = discover_pcscf(
        &bearer.settings,
        &device_identity.ims.home_domain,
        device_identity
            .effective_ims
            .pcscf
            .as_ref()
            .map(|field| field.value.as_str()),
        local_addr,
    )
    .await?;
    route_pcscf(bearer, pcscf).await?;
    runtime
        .update(|state| {
            state.stage = VolteStage::RegisterInitial;
            state.pcscf = Some(pcscf.to_string());
        })
        .await;

    let route = ImsRoute {
        local_addr: SocketAddr::new(local_addr, 0),
        pcscf_addr: pcscf_socket(pcscf),
        transport: SipTransport::Udp,
    };
    let profile = device_identity.profile;
    let sip_instance = sip_instance_for_profile(
        profile,
        device_identity.effective_device_identity.imei.as_deref(),
    );
    let mut register_variants = register_variants(profile).into_iter().peekable();
    let mut last_error = None;
    let mut pending_variant = None;
    while pending_variant.is_some() || register_variants.peek().is_some() {
        let mut variant = match pending_variant.take() {
            Some(variant) => variant,
            None => register_variants
                .next()
                .expect("REGISTER variant iterator was checked before use"),
        };
        variant.policy.include_video_feature = video_capability_enabled;
        let mut channel = VolteSipChannel::bind(route, Some(&bearer.interface), None)
            .map_err(map_channel_error)?;
        let receive_port = channel
            .reserve_security_receive_port()
            .map_err(map_channel_error)?;
        let ids = RequestIds::fresh(1);
        let offered_binding = offered_security(channel.route().local_addr.port(), receive_port);
        // TS 24.229 / RFC 3329: the initial REGISTER already advertises the
        // full ipsec-3gpp offer (alg/ealg/prot/mod + client SPI/ports). The
        // 401 then supplies the server binding; the authenticated REGISTER
        // repeats the identical Security-Client and mirrors Security-Server in
        // Security-Verify. A bare "Security-Client: ipsec-3gpp" is rejected by
        // strict P-CSCFs ("400 Bad header field: security-client").
        let negotiated_security = variant
            .security_client_offer
            .build(offered_binding, profile);
        let request_uri = sip::register_request_uri_with_target(
            profile,
            effective_register_target(&device_identity.effective_ims),
            &channel.route(),
        );
        let initial_authorization = variant.authorization.build(
            &device_identity.effective_ims.realm.value,
            &device_identity.ims,
            &request_uri,
        );
        let initial_security_client = (profile.ims.register.sec_agree_mode != "disabled"
            && !profile.ims.register.security_client_mechanisms.is_empty())
        .then_some(negotiated_security.as_str());
        let initial = sip::build_register_from_profile_with_target(
            profile,
            effective_register_target(&device_identity.effective_ims),
            sip::RegisterPhase::Initial,
            &device_identity.ims,
            &channel.route(),
            &ids,
            profile.ims.register.expires_seconds,
            initial_authorization.as_deref(),
            initial_security_client,
            None,
            &sip_instance,
            variant.policy,
        );
        log_volte_register_request_metadata(variant, &channel, &initial);
        runtime
            .record_attempt(
                VolteStage::RegisterInitial,
                Some(ip_family_name(local_addr)),
                "started",
                None,
                Some(format!("register_variant={}", variant.label)),
            )
            .await;
        let mut authenticator = VolteRegisterAuthenticator::new(
            device_identity.ims.clone(),
            ids,
            sip_instance.clone(),
            offered_binding,
            negotiated_security,
            channel.route(),
            device.clone(),
            runtime.clone(),
            false,
            device_identity.aka_aid.clone(),
            variant.policy,
            profile,
            device_identity.effective_ims.clone(),
        );
        let registration = match run_register_observed(&mut channel, &initial, &mut authenticator)
            .await
        {
            Ok(registration) => registration,
            Err(failure) => {
                log_volte_register_failure_metadata(variant, &failure);
                if let Some(plan) = authenticator.xfrm_plan.as_ref() {
                    ipsec::uninstall_plan(plan);
                }
                let error = map_register_failure(&failure);
                runtime
                    .record_attempt(
                        VolteStage::RegisterInitial,
                        Some(ip_family_name(local_addr)),
                        "failed",
                        Some(&error),
                        Some(format!("register_variant={}", variant.label)),
                    )
                    .await;
                let next_variant_available = register_variants.peek().is_some();
                if let Some(upgraded_variant) = sec_agree_retry_variant(variant, &failure) {
                    last_error = Some(error);
                    pending_variant = Some(upgraded_variant);
                    continue;
                }
                if let Some(spaced_security_variant) =
                    sec_agree_spaced_security_retry_variant(variant, &failure)
                {
                    last_error = Some(error);
                    pending_variant = Some(spaced_security_variant);
                    continue;
                }
                if let Some(compact_security_variant) =
                    sec_agree_compact_security_retry_variant(variant, &failure)
                {
                    last_error = Some(error);
                    pending_variant = Some(compact_security_variant);
                    continue;
                }
                if let Some(require_only_variant) =
                    sec_agree_require_only_retry_variant(variant, &failure)
                {
                    last_error = Some(error);
                    pending_variant = Some(require_only_variant);
                    continue;
                }
                if next_variant_available && sec_agree_require_only_was_rejected(variant, &failure)
                {
                    last_error = Some(error);
                    continue;
                }
                if next_variant_available
                    && failure.auth_rounds == 0
                    && register_failure_status(&failure) == Some(400)
                {
                    last_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        };
        runtime
            .record_attempt(
                VolteStage::Registered,
                Some(ip_family_name(local_addr)),
                "succeeded",
                None,
                Some(format!("register_variant={}", variant.label)),
            )
            .await;
        let registered = RegisteredImsContext::from_response(
            ImsRegistrationAccess::Volte,
            &registration.response,
            profile.ims.register.expires_seconds,
        );
        let associated_uri = registered.default_associated_uri();
        let mut registered_identity = device_identity.ims.clone();
        if let Some(uri) = associated_uri {
            // The network-provided default public user identity is authoritative
            // after REGISTER. In particular, operators commonly authenticate
            // with the IMSI-derived IMPU but require later requests to use the
            // MSISDN-associated IMPU.
            registered_identity.public_uri = uri.to_string();
        }
        tracing::info!(
            register_variant = variant.label,
            service_route_present = registered.service_route.is_some(),
            associated_uri_present = associated_uri.is_some(),
            "VoLTE IMS registration routing identities captured"
        );
        if authenticator.mode == RegistrationMode::Udp {
            runtime
                .update(|state| state.stage = VolteStage::RegisterUdp)
                .await;
        }
        return Ok(VolteLiveSession {
            channel,
            identity: registered_identity,
            registration: registered,
            bearer: bearer.clone(),
            pcscf: pcscf_socket(pcscf),
            ip_family: ip_family_name(local_addr),
            xfrm_plan: authenticator.xfrm_plan,
            // Attached by `connect_inner`, which owns it until the session is known
            // to be good.
            native_bearer: None,
            data_slot_mode: DataSlotMode::PrimaryImsOnly,
            pcscf_reporting_cid: None,
            ims_profile_lease: None,
            register_ids: authenticator.ids.clone(),
            next_register_cseq: authenticator
                .ids
                .cseq
                .saturating_add(u32::from(registration.auth_rounds))
                .saturating_add(1),
            sip_instance: authenticator.sip_instance,
            security_binding: authenticator.offered_security_binding,
            register_variant: variant,
            device: device.clone(),
            aka_aid: device_identity.aka_aid.clone(),
            profile,
            effective_ims: device_identity.effective_ims.clone(),
            voice_calls: HashMap::new(),
            mwi_subscription: None,
        });
    }
    Err(last_error.unwrap_or_else(|| VolteError::new(code::REGISTER_INITIAL_UNEXPECTED_STATUS)))
}

pub async fn disconnect_live_for_line(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    reason: &str,
) -> VolteRuntimeStatus {
    if let Some(listener) = live.listener.lock().await.take() {
        listener.abort();
    }
    let unregister = unregister_live_session(live, runtime).await;
    tracing::info!(result = ?unregister, "VoLTE explicit IMS unregister finished");
    cleanup_live_session(live).await;
    runtime.reset_runtime(reason).await;
    runtime.status().await
}

async fn unregister_live_session(
    live: &VolteLiveHandle,
    runtime: &VolteRuntime,
) -> UnregisterResult {
    let mut sessions = live.session.lock().await;
    let Some(session) = sessions.as_mut() else {
        return UnregisterResult::AlreadyExpired;
    };
    if session
        .registration
        .registered_at
        .elapsed()
        .is_ok_and(|age| age >= session.registration.lease.expires_after)
    {
        return UnregisterResult::AlreadyExpired;
    }

    let mut ids = session.register_ids.clone();
    ids.cseq = session.next_register_cseq;
    let security_verify = session.channel.security_verify().map(str::to_string);
    let request_uri = sip::register_request_uri_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        &session.channel.route(),
    );
    let initial_authorization = session.register_variant.authorization.build(
        &session.effective_ims.realm.value,
        &session.identity,
        &request_uri,
    );
    let register_policy = sip::RegisterRequestPolicy {
        require_sec_agree: security_verify.is_some(),
        ..session.register_variant.policy
    };
    let request = sip::build_register_from_profile_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        sip::RegisterPhase::Refresh,
        &session.identity,
        &session.channel.route(),
        &ids,
        0,
        initial_authorization.as_deref(),
        None,
        security_verify.as_deref(),
        &session.sip_instance,
        register_policy,
    );
    let mut authenticator = VolteRegisterAuthenticator::new(
        session.identity.clone(),
        ids,
        session.sip_instance.clone(),
        session.security_binding,
        session
            .register_variant
            .security_client_offer
            .build(session.security_binding, session.profile),
        session.channel.route(),
        session.device.clone(),
        runtime.clone(),
        true,
        session.aka_aid.clone(),
        register_policy,
        session.profile,
        session.effective_ims.clone(),
    )
    .with_expires_seconds(0);
    run_unregister(&mut session.channel, &request, &mut authenticator).await
}

async fn start_live_listener(
    live: VolteLiveHandle,
    line_id: String,
    runtime: Arc<VolteRuntime>,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
    generation: u64,
    dedupe_enabled: bool,
) {
    let mut listener = live.listener.lock().await;
    if let Some(previous) = listener.take() {
        previous.abort();
    }
    let receive_live = live.clone();
    *listener = Some(tokio::spawn(async move {
        live_receive_loop(
            receive_live,
            line_id,
            runtime,
            database,
            notification_sender,
            generation,
            dedupe_enabled,
        )
        .await;
    }));
}

async fn start_volte_mwi_subscription(live: &VolteLiveHandle) {
    let Some(runtime) = live.supplementary_runtime() else {
        return;
    };
    let is_refresh = {
        let sessions = live.session.lock().await;
        sessions
            .as_ref()
            .is_some_and(|session| session.mwi_subscription.is_some())
    };
    if is_refresh {
        if !runtime
            .owns_mwi_subscription(ImsRegistrationAccess::Volte)
            .await
        {
            if let Some(session) = live.session.lock().await.as_mut() {
                session.mwi_subscription = None;
            }
            return;
        }
    } else {
        runtime
            .begin_mwi_subscription(ImsRegistrationAccess::Volte)
            .await;
    }
    let send_result = {
        let mut sessions = live.session.lock().await;
        let Some(session) = sessions.as_mut() else {
            return;
        };
        let ids = match session.mwi_subscription.take() {
            Some(previous) => SubscribeIds {
                branch: sip::new_branch(),
                from_tag: previous.ids.from_tag,
                to_tag: previous.ids.to_tag,
                call_id: previous.ids.call_id,
                cseq: previous.ids.cseq.saturating_add(1),
            },
            None => {
                let request_ids = RequestIds::fresh(1);
                SubscribeIds {
                    branch: sip::new_branch(),
                    from_tag: request_ids.from_tag,
                    to_tag: None,
                    call_id: request_ids.call_id,
                    cseq: request_ids.cseq,
                }
            }
        };
        let mut access_headers = vec![SipHeader::new("P-Access-Network-Info", sip::PANI_EUTRAN)];
        if let Some(value) = session.channel.security_verify() {
            access_headers.push(SipHeader::new("Security-Verify", value));
        }
        let frame = build_mwi_subscribe(
            &session.identity,
            &session.channel.route(),
            &session.registration,
            &ids,
            MWI_SUBSCRIBE_EXPIRES_SECONDS,
            sip::USER_AGENT,
            &access_headers,
        );
        match session.channel.send_sip(&frame).await {
            Ok(()) => {
                let refresh_seconds =
                    (u64::from(MWI_SUBSCRIBE_EXPIRES_SECONDS).saturating_mul(11) / 12).max(1);
                session.mwi_subscription = Some(MwiSubscription {
                    ids,
                    refresh_at: tokio::time::Instant::now() + Duration::from_secs(refresh_seconds),
                    authenticated: false,
                });
                Ok(())
            }
            Err(error) => {
                session.mwi_subscription = None;
                Err(error)
            }
        }
    };
    if let Err(error) = send_result {
        runtime
            .fail_mwi_subscription(ImsRegistrationAccess::Volte, error.code())
            .await;
    }
}

async fn live_receive_loop(
    live: VolteLiveHandle,
    line_id: String,
    runtime: Arc<VolteRuntime>,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
    generation: u64,
    dedupe_enabled: bool,
) {
    let mut reassembler = MtReassembler::new();
    let mut operator_commands = live.operator.subscribe_commands();
    start_volte_mwi_subscription(&live).await;
    let mut refresh_at = {
        let sessions = live.session.lock().await;
        sessions
            .as_ref()
            .map(|session| tokio::time::Instant::now() + session.registration.lease.refresh_after)
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(60))
    };
    // The native IMS bearer retains its WDS CID on the secondary QMI endpoint.
    // REGISTER refresh remains the end-to-end bearer health signal because it
    // covers the IMS IP path and SIP service, not only WDS packet status.
    loop {
        if runtime.generation() != generation {
            break;
        }
        if let Err(error) = expire_volte_renegotiations(&live).await {
            tracing::warn!(error = %error, "VoLTE re-INVITE timeout handling failed");
            cleanup_live_session(&live).await;
            break;
        }
        let mwi_refresh_due = {
            let sessions = live.session.lock().await;
            sessions
                .as_ref()
                .and_then(|session| session.mwi_subscription.as_ref())
                .is_some_and(|subscription| tokio::time::Instant::now() >= subscription.refresh_at)
        };
        if mwi_refresh_due {
            start_volte_mwi_subscription(&live).await;
            continue;
        }
        if tokio::time::Instant::now() >= refresh_at {
            let refresh_result = {
                let mut sessions = live.session.lock().await;
                match sessions.as_mut() {
                    Some(session) => refresh_live_registration(session, &runtime).await,
                    None => break,
                }
            };
            match refresh_result.outcome {
                RegistrationRefreshResult::Refreshed(registration) => {
                    refresh_at = tokio::time::Instant::now() + registration.lease.refresh_after;
                    continue;
                }
                RegistrationRefreshResult::RebuildAccess(loss_reason) => {
                    let error = refresh_result.error.unwrap_or_else(|| {
                        VolteError::with_detail(
                            code::REGISTER_AUTH_UNEXPECTED_STATUS,
                            loss_reason.as_str(),
                        )
                    });
                    tracing::warn!(
                        error = %error,
                        registration_loss = loss_reason.as_str(),
                        "VoLTE REGISTER refresh failed; rebuilding session"
                    );
                    runtime
                        .update(|state| {
                            state.phase = VoltePhase::Degraded;
                            state.last_error = Some(format!(
                                "volte_register_refresh_failed:{}:{error}",
                                loss_reason.as_str()
                            ));
                            state.last_failure_at = Some(now());
                            state.reconnect_count = state.reconnect_count.saturating_add(1);
                        })
                        .await;
                    cleanup_live_session(&live).await;
                    break;
                }
            }
        }
        let input = {
            let mut sessions = live.session.lock().await;
            let Some(session) = sessions.as_mut() else {
                break;
            };
            tokio::select! {
                frame = session.channel.recv_sip(Duration::from_secs(1)) => LiveLoopInput::Sip(frame),
                command = operator_commands.recv() => LiveLoopInput::Command(Box::new(command)),
            }
        };
        match input {
            LiveLoopInput::Command(command) => match *command {
                Ok(command) => {
                    if let Err(error) = handle_operator_command(&live, &runtime, command).await {
                        tracing::warn!(error = %error, "VoLTE operator command failed");
                    }
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "VoLTE operator command receiver lagged");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            LiveLoopInput::Sip(Err(error)) if error.code() == "volte_channel_read_timeout" => {
                continue
            }
            LiveLoopInput::Sip(Err(error)) => {
                tracing::warn!(error = %error, "VoLTE protected SIP receive failed");
                runtime
                    .update(|state| {
                        state.phase = VoltePhase::Degraded;
                        state.last_error = Some(error.to_string());
                        state.last_failure_at = Some(now());
                    })
                    .await;
                cleanup_live_session(&live).await;
                break;
            }
            LiveLoopInput::Sip(Ok(frame)) => {
                runtime.update(|state| state.last_rx_at = Some(now())).await;
                if let Err(error) = handle_live_frame(
                    LiveFrameContext {
                        live: &live,
                        line_id: &line_id,
                        runtime: &runtime,
                        database: &database,
                        notification_sender: &notification_sender,
                        dedupe_enabled,
                    },
                    &mut reassembler,
                    &frame,
                )
                .await
                {
                    tracing::warn!(error = %error, "VoLTE protected SIP frame handling failed");
                }
            }
        }
    }
}

/// Roll back an unanswered re-INVITE without touching the confirmed dialog or
/// active audio relay. Network-initiated re-INVITEs receive a timeout response;
/// trunk-initiated re-INVITEs receive a local 408 event.
async fn expire_volte_renegotiations(live: &VolteLiveHandle) -> Result<(), VolteError> {
    let now = Instant::now();
    let mut trunk_timeouts = Vec::new();
    let mut transfer_timeouts = Vec::new();
    let mut sessions = live.session.lock().await;
    let Some(session) = sessions.as_mut() else {
        return Ok(());
    };
    let mut network_responses = Vec::new();
    for (call_id, call) in &mut session.voice_calls {
        if call
            .renegotiation_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            call.renegotiation_deadline = None;
            call.rollback_media_update();
            if let Some(request) = call.pending_operator_reinvite.take() {
                network_responses.push(sip::build_response(
                    &request,
                    504,
                    "Server Time-out",
                    Some(&call.dialog.local_tag),
                    None,
                    None,
                ));
            }
            if call.pending_asterisk_reinvite {
                call.pending_asterisk_reinvite = false;
                trunk_timeouts.push(call_id.clone());
            }
        }
        if call
            .transfer_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            call.transfer_deadline = None;
            if let Some(transfer) = call.transfer.as_mut() {
                let _ = transfer.on_refer_response(408);
            }
            transfer_timeouts.push(call_id.clone());
        }
    }
    for response in network_responses {
        session
            .channel
            .send_sip(&response)
            .await
            .map_err(map_channel_error)?;
    }
    drop(sessions);
    for call_id in trunk_timeouts {
        live.operator.send_event(OperatorEvent::Rejected {
            call_id,
            status: 408,
        });
    }
    for call_id in transfer_timeouts {
        live.operator.send_event(OperatorEvent::TransferResponse {
            call_id,
            status: 408,
        });
    }
    Ok(())
}

async fn refresh_live_registration(
    session: &mut VolteLiveSession,
    runtime: &VolteRuntime,
) -> VolteRefreshAttempt {
    runtime
        .update(|state| state.stage = VolteStage::RegisterRefresh)
        .await;
    runtime
        .record_attempt(
            VolteStage::RegisterRefresh,
            Some(session.ip_family),
            "started",
            None,
            None,
        )
        .await;

    let mut ids = session.register_ids.clone();
    ids.cseq = session.next_register_cseq;
    let security_verify = session.channel.security_verify().map(str::to_string);
    let require_sec_agree = security_verify.is_some();
    let request_uri = sip::register_request_uri_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        &session.channel.route(),
    );
    let initial_authorization = session.register_variant.authorization.build(
        &session.effective_ims.realm.value,
        &session.identity,
        &request_uri,
    );
    let register_policy = sip::RegisterRequestPolicy {
        require_sec_agree,
        ..session.register_variant.policy
    };
    let initial = sip::build_register_from_profile_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        sip::RegisterPhase::Refresh,
        &session.identity,
        &session.channel.route(),
        &ids,
        session.profile.ims.register.expires_seconds,
        initial_authorization.as_deref(),
        None,
        security_verify.as_deref(),
        &session.sip_instance,
        register_policy,
    );
    let mut authenticator = VolteRegisterAuthenticator::new(
        session.identity.clone(),
        ids.clone(),
        session.sip_instance.clone(),
        session.security_binding.clone(),
        session
            .register_variant
            .security_client_offer
            .build(session.security_binding, session.profile),
        session.channel.route(),
        session.device.clone(),
        runtime.clone(),
        true,
        session.aka_aid.clone(),
        register_policy,
        session.profile,
        session.effective_ims.clone(),
    );
    let registration =
        match run_register_observed(&mut session.channel, &initial, &mut authenticator).await {
            Ok(registration) => registration,
            Err(failure) => {
                let loss_reason = RegistrationLossReason::from_register_failure(&failure);
                let error = map_register_failure(&failure);
                runtime
                    .record_attempt(
                        VolteStage::RegisterRefresh,
                        Some(session.ip_family),
                        "failed",
                        Some(&error),
                        None,
                    )
                    .await;
                return VolteRefreshAttempt {
                    outcome: RegistrationRefreshResult::RebuildAccess(loss_reason),
                    error: Some(error),
                };
            }
        };

    session.next_register_cseq = ids
        .cseq
        .saturating_add(u32::from(registration.auth_rounds))
        .saturating_add(1);
    let registered = RegisteredImsContext::from_response(
        ImsRegistrationAccess::Volte,
        &registration.response,
        session.profile.ims.register.expires_seconds,
    );
    if let Some(uri) = registered.default_associated_uri() {
        session.identity.public_uri = uri.to_string();
    }
    session.registration = registered.clone();
    runtime
        .record_attempt(
            VolteStage::RegisterRefresh,
            Some(session.ip_family),
            "succeeded",
            None,
            Some(format!("cseq={}", ids.cseq)),
        )
        .await;
    runtime
        .update(|state| {
            state.phase = VoltePhase::Registered;
            state.stage = VolteStage::Registered;
            state.last_error = None;
            state.last_register_refresh_at = Some(now());
            state.last_tx_at = Some(now());
            state.register_refresh_count = state.register_refresh_count.saturating_add(1);
        })
        .await;
    VolteRefreshAttempt {
        outcome: RegistrationRefreshResult::Refreshed(registered),
        error: None,
    }
}

struct VolteRefreshAttempt {
    outcome: RegistrationRefreshResult,
    error: Option<VolteError>,
}

async fn cleanup_live_session(live: &VolteLiveHandle) {
    live.operator.set_ready(false);
    let session = live.session.lock().await.take();
    if let Some(session) = session {
        if let Some(plan) = session.xfrm_plan.as_ref() {
            ipsec::uninstall_plan(plan);
        }
        teardown_bearer_network(&session.bearer).await;
        // A native bearer has no ModemManager object: its WDS session must be
        // stopped through the handle we kept, or the PDP context stays up on the
        // modem after the line is disconnected. `disconnect_bearer` ignores
        // native paths, so this is the only path that actually releases it.
        match session.native_bearer {
            Some(native) => native_bearer::release_native_ims_bearer(native).await,
            None => disconnect_bearer(&session.bearer.path).await,
        }
        disable_pcscf_reporting(&session.device.modem_id, session.pcscf_reporting_cid).await;
        cleanup_ims_profile_lease(session.ims_profile_lease).await;
    }
    if let Some(runtime) = live.supplementary_runtime() {
        runtime
            .clear_registration(ImsRegistrationAccess::Volte)
            .await;
    }
    cleanup_retained_failed_bearer(live).await;
}

async fn cleanup_retained_failed_bearer(live: &VolteLiveHandle) {
    let Some(retained) = live.failed_bearer.lock().await.take() else {
        return;
    };
    let elapsed = retained.retained_at.elapsed();
    if elapsed < FAILED_BEARER_MIN_RETENTION {
        tokio::time::sleep(FAILED_BEARER_MIN_RETENTION - elapsed).await;
    }
    teardown_bearer_network(&retained.bearer).await;
    match retained.native_bearer {
        Some(native) => native_bearer::release_native_ims_bearer(native).await,
        None => disconnect_bearer(&retained.bearer.path).await,
    }
    disable_pcscf_reporting(&retained.modem_id, retained.pcscf_reporting_cid).await;
    cleanup_ims_profile_lease(retained.ims_profile_lease).await;
}

async fn cleanup_ims_profile_lease(lease: Option<ImsProfileLease>) {
    if let Some(lease) = lease {
        lease.cleanup().await;
    }
}

async fn disable_pcscf_reporting(modem: &str, cid: Option<u8>) {
    let Some(cid) = cid else {
        return;
    };
    if let Err(error) = set_pcscf_reporting(modem, cid, false).await {
        tracing::warn!(cid, error = %error, "Failed to restore IMS P-CSCF reporting state");
    }
}

struct LiveFrameContext<'a> {
    live: &'a VolteLiveHandle,
    line_id: &'a str,
    runtime: &'a Arc<VolteRuntime>,
    database: &'a Arc<Database>,
    notification_sender: &'a Arc<NotificationSender>,
    dedupe_enabled: bool,
}

enum LiveLoopInput {
    Sip(Result<Vec<u8>, ImsError>),
    Command(Box<Result<OperatorCommand, tokio::sync::broadcast::error::RecvError>>),
}

async fn handle_operator_command(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    command: OperatorCommand,
) -> Result<(), VolteError> {
    let call_id = operator_command_call_id(&command).to_string();
    let initial_call = matches!(&command, OperatorCommand::StartCall { .. });
    let renegotiate = matches!(&command, OperatorCommand::Renegotiate { .. });
    let transfer = matches!(&command, OperatorCommand::TransferCall { .. });
    let result = handle_operator_command_inner(live, runtime, command).await;
    if result.is_err() && initial_call {
        if result
            .as_ref()
            .is_err_and(|error| error.code() == "volte_concurrent_call_limit")
        {
            live.operator.send_event(OperatorEvent::Rejected {
                call_id: call_id.clone(),
                status: 486,
            });
        } else {
            live.operator.send_event(OperatorEvent::Unavailable {
                call_id: call_id.clone(),
            });
        }
    }
    if result.is_err() && renegotiate {
        let mut sessions = live.session.lock().await;
        if let Some(call) = sessions
            .as_mut()
            .and_then(|session| session.voice_calls.get_mut(&call_id))
        {
            call.pending_asterisk_reinvite = false;
            call.rollback_media_update();
            call.renegotiation_deadline = None;
        }
    }
    if result.is_err() && transfer {
        let status = result
            .as_ref()
            .err()
            .map(VolteError::code)
            .map(|code| {
                if code.ends_with("_pending") {
                    491
                } else if code.ends_with("_unknown") || code.ends_with("_not_confirmed") {
                    481
                } else {
                    500
                }
            })
            .unwrap_or(500);
        live.operator
            .send_event(OperatorEvent::TransferResponse { call_id, status });
    }
    result
}

async fn handle_operator_command_inner(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    command: OperatorCommand,
) -> Result<(), VolteError> {
    let mut sessions = live.session.lock().await;
    let session = sessions
        .as_mut()
        .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
    let frame = match command {
        OperatorCommand::StartCall {
            call_id,
            callee,
            trunk_local_ip,
            offer,
            ..
        } => {
            if session.voice_calls.contains_key(&call_id) {
                return Err(VolteError::new("volte_voice_call_duplicate"));
            }
            if session.voice_calls.len() >= MAX_CONCURRENT_CALLS {
                return Err(VolteError::new("volte_concurrent_call_limit"));
            }
            if offer.video.is_some() && !live.operator.video_enabled() {
                return Err(VolteError::new("vilte_feature_disabled"));
            }
            let callee_uri = normalize_operator_callee(&callee, &session.identity.home_domain)?;
            let relay =
                PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
                    .await
                    .map_err(|error| {
                        VolteError::with_detail("volte_rtp_bind_failed", error.to_string())
                    })?;
            let operator_local = relay.operator_local_addr().map_err(|error| {
                VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
            })?;
            let internal_local = relay.internal_local_addr().map_err(|error| {
                VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
            })?;
            let (video_relay, operator_video_local, internal_video_local) = if offer.video.is_some()
            {
                let relay =
                    PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
                        .await
                        .map_err(|error| {
                            VolteError::with_detail("vilte_rtp_bind_failed", error.to_string())
                        })?;
                let operator_local = relay.operator_local_addr().map_err(|error| {
                    VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
                })?;
                let internal_local = relay.internal_local_addr().map_err(|error| {
                    VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
                })?;
                (Some(relay), Some(operator_local), Some(internal_local))
            } else {
                (None, None, None)
            };
            let body = relay_media_sdp(&offer, operator_local, operator_video_local);
            let dialog = sip::DialogIds::fresh();
            let frame = sip::build_invite(
                &session.identity,
                &session.channel.route(),
                session.registration.service_route.as_deref(),
                &dialog,
                &callee_uri,
                body.as_bytes(),
                session.channel.security_verify(),
            );
            let invite_branch = top_via_branch(&frame)
                .ok_or_else(|| VolteError::new("volte_voice_invite_branch_missing"))?;
            session.voice_calls.insert(
                call_id,
                LiveVoiceCall {
                    direction: LiveVoiceDirection::MobileOriginated,
                    next_cseq: dialog.cseq.saturating_add(1),
                    dialog,
                    callee_uri,
                    invite_branch,
                    initial_invite: None,
                    internal_offer: offer,
                    operator_local,
                    internal_local,
                    pending_relay: Some(relay),
                    active_relay: None,
                    ip_answer_wait_armed: false,
                    operator_answered: false,
                    media_metrics: Some(live.operator.media_metrics()),
                    pending_operator_reinvite: None,
                    pending_asterisk_reinvite: false,
                    pending_video_relay: video_relay,
                    active_video_relay: None,
                    operator_video_local,
                    internal_video_local,
                    pending_media_rollback: None,
                    renegotiation_deadline: None,
                    early_answer: None,
                    transfer: None,
                    transfer_deadline: None,
                },
            );
            frame
        }
        OperatorCommand::CancelCall { call_id } => {
            let call = session
                .voice_calls
                .remove(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            sip::build_cancel(
                &session.identity,
                &session.channel.route(),
                session.registration.service_route.as_deref(),
                &call.dialog,
                &call.callee_uri,
                &call.invite_branch,
            )
        }
        OperatorCommand::HangupCall { call_id } => {
            let call = session
                .voice_calls
                .remove(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            if call.dialog.remote_tag.is_some() {
                sip::build_bye(
                    &session.identity,
                    &session.channel.route(),
                    session.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.callee_uri,
                    call.next_cseq,
                )
            } else {
                sip::build_cancel(
                    &session.identity,
                    &session.channel.route(),
                    session.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.callee_uri,
                    &call.invite_branch,
                )
            }
        }
        OperatorCommand::SendDtmf { call_id, signal } => {
            let call = session
                .voice_calls
                .get_mut(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            let cseq = call.next_cseq;
            call.next_cseq = call.next_cseq.saturating_add(1);
            sip::build_dtmf_info(
                &session.identity,
                &session.channel.route(),
                session.registration.service_route.as_deref(),
                &call.dialog,
                &call.callee_uri,
                cseq,
                signal.digit,
                signal.duration_ms,
            )?
        }
        OperatorCommand::TransferCall { call_id, refer_to } => {
            let operator_refer_to =
                normalize_operator_callee(&refer_to, &session.identity.home_domain)?;
            let call = session
                .voice_calls
                .get_mut(&call_id)
                .ok_or_else(|| VolteError::new("volte_transfer_call_unknown"))?;
            if !call.operator_answered || call.dialog.remote_tag.is_none() {
                return Err(VolteError::new("volte_transfer_call_not_confirmed"));
            }
            if call
                .transfer
                .as_ref()
                .is_some_and(|transfer| !transfer.state().is_terminal())
            {
                return Err(VolteError::new("volte_transfer_pending"));
            }
            let cseq = call.next_cseq;
            call.next_cseq = call.next_cseq.saturating_add(1);
            let to_value = format!(
                "<{}>;tag={}",
                call.callee_uri,
                call.dialog.remote_tag.as_deref().unwrap_or_default()
            );
            let mut access_headers = vec![
                SipHeader::new("P-Access-Network-Info", sip::PANI_EUTRAN),
                SipHeader::new("User-Agent", sip::USER_AGENT),
            ];
            if let Some(value) = session.channel.security_verify() {
                access_headers.push(SipHeader::new("Security-Verify", value));
            }
            let branch = sip::new_branch();
            let frame = build_dialog_refer(
                &session.identity,
                &session.channel.route(),
                &session.registration,
                &DialogReferRequest {
                    request_uri: &call.callee_uri,
                    branch: &branch,
                    from_uri: &session.identity.public_uri,
                    from_tag: &call.dialog.local_tag,
                    to_value: &to_value,
                    call_id: &call.dialog.call_id,
                    cseq,
                    refer_to: &operator_refer_to,
                    referred_by: Some(&session.identity.public_uri),
                },
                &access_headers,
            )
            .map_err(|error| {
                VolteError::with_detail("volte_transfer_request_invalid", error.to_string())
            })?;
            call.transfer = Some(DialogTransfer::for_refer_cseq(cseq));
            call.transfer_deadline = Some(Instant::now() + REFER_RESPONSE_TIMEOUT);
            frame
        }
        OperatorCommand::Renegotiate {
            call_id,
            trunk_local_ip,
            offer,
        } => {
            if offer.video.is_some() && !live.operator.video_enabled() {
                return Err(VolteError::new("vilte_feature_disabled"));
            }
            let operator_ip = session.channel.route().local_addr.ip();
            let pending = PendingRtpRelay::bind(operator_ip, trunk_local_ip)
                .await
                .map_err(|error| {
                    VolteError::with_detail("volte_rtp_bind_failed", error.to_string())
                })?;
            let operator_local = pending.operator_local_addr().map_err(|error| {
                VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
            })?;
            let internal_local = pending.internal_local_addr().map_err(|error| {
                VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
            })?;
            let (video_relay, operator_video_local, internal_video_local) = if offer.video.is_some()
            {
                let relay = PendingRtpRelay::bind(operator_ip, trunk_local_ip)
                    .await
                    .map_err(|error| {
                        VolteError::with_detail("vilte_rtp_bind_failed", error.to_string())
                    })?;
                let operator_local = relay.operator_local_addr().map_err(|error| {
                    VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
                })?;
                let internal_local = relay.internal_local_addr().map_err(|error| {
                    VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
                })?;
                (Some(relay), Some(operator_local), Some(internal_local))
            } else {
                (None, None, None)
            };
            let call = session
                .voice_calls
                .get_mut(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            if call.pending_operator_reinvite.is_some() || call.pending_asterisk_reinvite {
                return Err(VolteError::new("volte_voice_reinvite_pending"));
            }
            call.dialog.cseq = call.next_cseq;
            call.next_cseq = call.next_cseq.saturating_add(1);
            call.stage_media_update(
                offer.clone(),
                pending,
                operator_local,
                internal_local,
                video_relay,
                operator_video_local,
                internal_video_local,
            );
            call.pending_asterisk_reinvite = true;
            call.renegotiation_deadline = Some(Instant::now() + REINVITE_TIMEOUT);
            let body = relay_media_sdp(&offer, call.operator_local, call.operator_video_local);
            sip::build_reinvite(
                &session.identity,
                &session.channel.route(),
                session.registration.service_route.as_deref(),
                &call.dialog,
                &call.callee_uri,
                body.as_bytes(),
                session.channel.security_verify(),
            )
        }
        OperatorCommand::AcceptRenegotiation { call_id, body } => {
            let call = session
                .voice_calls
                .get_mut(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            let answer = prepare_incoming_media(call, &body)?;
            let request = call
                .pending_operator_reinvite
                .take()
                .ok_or_else(|| VolteError::new("volte_voice_reinvite_not_pending"))?;
            call.commit_media_update();
            call.renegotiation_deadline = None;
            let contact = ims_contact(&session.identity, &session.channel.route());
            sip::build_response(
                &request,
                200,
                "OK",
                Some(&call.dialog.local_tag),
                Some(&contact),
                Some(answer.as_bytes()),
            )
        }
        OperatorCommand::RejectRenegotiation { call_id, status } => {
            let call = session
                .voice_calls
                .get_mut(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            let request = call
                .pending_operator_reinvite
                .take()
                .ok_or_else(|| VolteError::new("volte_voice_reinvite_not_pending"))?;
            call.rollback_media_update();
            call.renegotiation_deadline = None;
            sip::build_response(
                &request,
                status,
                ims_reason(status),
                Some(&call.dialog.local_tag),
                None,
                None,
            )
        }
        OperatorCommand::ReportProvisional {
            call_id,
            status,
            body,
        } => {
            let (request, local_tag, answer) = {
                let call = session
                    .voice_calls
                    .get_mut(&call_id)
                    .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
                if call.direction != LiveVoiceDirection::MobileTerminated {
                    return Err(VolteError::new("volte_voice_direction_mismatch"));
                }
                if call.operator_answered {
                    return Ok(());
                }
                let request = call
                    .initial_invite
                    .clone()
                    .ok_or_else(|| VolteError::new("volte_voice_initial_invite_missing"))?;
                let local_tag = call.dialog.local_tag.clone();
                let answer = body
                    .as_deref()
                    .map(|body| prepare_incoming_media(call, body))
                    .transpose();
                (request, local_tag, answer)
            };
            match answer {
                Ok(answer) => sip::build_response(
                    &request,
                    status,
                    ims_reason(status),
                    Some(&local_tag),
                    None,
                    answer.as_deref().map(str::as_bytes),
                ),
                Err(error) => {
                    session.voice_calls.remove(&call_id);
                    live.operator.send_event(OperatorEvent::Cancelled {
                        call_id: call_id.clone(),
                    });
                    tracing::warn!(error = %error, "Rejected unusable Asterisk early media");
                    sip::build_response(
                        &request,
                        488,
                        "Not Acceptable Here",
                        Some(&local_tag),
                        None,
                        None,
                    )
                }
            }
        }
        OperatorCommand::AcceptCall { call_id, body } => {
            let contact = ims_contact(&session.identity, &session.channel.route());
            let (request, local_tag, operator_answered, answer) = {
                let call = session
                    .voice_calls
                    .get_mut(&call_id)
                    .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
                if call.direction != LiveVoiceDirection::MobileTerminated {
                    return Err(VolteError::new("volte_voice_direction_mismatch"));
                }
                let request = call
                    .initial_invite
                    .clone()
                    .ok_or_else(|| VolteError::new("volte_voice_initial_invite_missing"))?;
                let local_tag = call.dialog.local_tag.clone();
                let operator_answered = call.operator_answered;
                let answer = prepare_incoming_media(call, &body);
                (request, local_tag, operator_answered, answer)
            };
            match answer {
                Ok(_) if operator_answered => return Ok(()),
                Ok(answer) => sip::build_response(
                    &request,
                    200,
                    "OK",
                    Some(&local_tag),
                    Some(&contact),
                    Some(answer.as_bytes()),
                ),
                Err(error) => {
                    let call = session.voice_calls.remove(&call_id);
                    live.operator.send_event(OperatorEvent::Ended {
                        call_id: call_id.clone(),
                    });
                    tracing::warn!(error = %error, "Rejected unusable Asterisk answer");
                    if operator_answered {
                        let call =
                            call.ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
                        sip::build_bye(
                            &session.identity,
                            &session.channel.route(),
                            session.registration.service_route.as_deref(),
                            &call.dialog,
                            &call.callee_uri,
                            call.next_cseq,
                        )
                    } else {
                        sip::build_response(
                            &request,
                            488,
                            "Not Acceptable Here",
                            Some(&local_tag),
                            None,
                            None,
                        )
                    }
                }
            }
        }
        OperatorCommand::RejectCall { call_id, status } => {
            let call = session
                .voice_calls
                .remove(&call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            if call.direction != LiveVoiceDirection::MobileTerminated {
                return Err(VolteError::new("volte_voice_direction_mismatch"));
            }
            if call.operator_answered {
                sip::build_bye(
                    &session.identity,
                    &session.channel.route(),
                    session.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.callee_uri,
                    call.next_cseq,
                )
            } else {
                let request = call
                    .initial_invite
                    .as_deref()
                    .ok_or_else(|| VolteError::new("volte_voice_initial_invite_missing"))?;
                sip::build_response(
                    request,
                    status,
                    ims_reason(status),
                    Some(&call.dialog.local_tag),
                    None,
                    None,
                )
            }
        }
    };
    session
        .channel
        .send_sip(&frame)
        .await
        .map_err(map_channel_error)?;
    runtime.update(|state| state.last_tx_at = Some(now())).await;
    Ok(())
}

fn operator_command_call_id(command: &OperatorCommand) -> &str {
    match command {
        OperatorCommand::StartCall { call_id, .. }
        | OperatorCommand::CancelCall { call_id }
        | OperatorCommand::HangupCall { call_id }
        | OperatorCommand::Renegotiate { call_id, .. }
        | OperatorCommand::AcceptRenegotiation { call_id, .. }
        | OperatorCommand::RejectRenegotiation { call_id, .. }
        | OperatorCommand::ReportProvisional { call_id, .. }
        | OperatorCommand::AcceptCall { call_id, .. }
        | OperatorCommand::RejectCall { call_id, .. }
        | OperatorCommand::SendDtmf { call_id, .. }
        | OperatorCommand::TransferCall { call_id, .. } => call_id,
    }
}

fn normalize_operator_callee(callee: &str, home_domain: &str) -> Result<String, VolteError> {
    let user = crate::connectivity::core::voice::normalize_ims_dial_user(callee)
        .map_err(|_| VolteError::new("volte_voice_callee_invalid"))?;
    Ok(format!("sip:{user}@{home_domain};user=phone"))
}

fn top_via_branch(frame: &[u8]) -> Option<String> {
    let via = sip::header_value(frame, "Via")?;
    via.split(';').find_map(|parameter| {
        parameter
            .trim()
            .strip_prefix("branch=")
            .map(ToOwned::to_owned)
    })
}

fn relay_audio_sdp(
    description: &SdpAudioDescription,
    dtmf: Option<&RtpTelephoneEvent>,
    local: SocketAddr,
) -> String {
    let mut audio = description.clone();
    audio.connection_addr = local.ip().to_string();
    audio.addr_type = if local.is_ipv4() {
        SdpAddrType::Ip4
    } else {
        SdpAddrType::Ip6
    };
    audio.media_port = local.port();
    let base = audio.to_sdp();
    let Some(dtmf) = dtmf else {
        return base;
    };
    let mut output = String::new();
    for line in base.lines() {
        if line.starts_with("m=audio ") {
            output.push_str(line);
            output.push(' ');
            output.push_str(&dtmf.payload_type.to_string());
            output.push_str("\r\n");
            continue;
        }
        if line.starts_with("a=send") || line == "a=recvonly" || line == "a=inactive" {
            output.push_str(&format!(
                "a=rtpmap:{} telephone-event/{}\r\n",
                dtmf.payload_type, dtmf.clock_rate
            ));
            if let Some(events) = dtmf.events.as_deref() {
                output.push_str(&format!("a=fmtp:{} {events}\r\n", dtmf.payload_type));
            }
        }
        output.push_str(line);
        output.push_str("\r\n");
    }
    output
}

fn relay_media_sdp(
    offer: &MediaOffer,
    audio_local: SocketAddr,
    video_local: Option<SocketAddr>,
) -> String {
    let mut audio = offer.audio.clone();
    audio.direction = audio.direction.for_peer();
    let mut output = relay_audio_sdp(&audio, offer.dtmf.rtp_event.as_ref(), audio_local);
    if let (Some(video), Some(local)) = (offer.video.as_ref(), video_local) {
        let mut description = video.description.clone();
        description.direction = description.direction.for_peer();
        description.media_port = local.port();
        description.connection_addr = Some(local.ip().to_string());
        description.addr_type = Some(if local.is_ipv4() {
            SdpAddrType::Ip4
        } else {
            SdpAddrType::Ip6
        });
        output.push_str(&description.media_lines());
    }
    output
}

async fn handle_operator_sip_frame(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    frame: &[u8],
) -> Result<bool, VolteError> {
    let Some(ims_call_id) = sip::header_value(frame, "Call-ID") else {
        return Ok(false);
    };
    let mut sessions = live.session.lock().await;
    let Some(session) = sessions.as_mut() else {
        return Ok(false);
    };
    let Some(trunk_call_id) = session
        .voice_calls
        .iter()
        .find(|(_, call)| call.dialog.call_id == ims_call_id)
        .map(|(call_id, _)| call_id.clone())
    else {
        if sip::is_request(frame, "INVITE") {
            return begin_incoming_operator_call(live, runtime, session, frame, ims_call_id).await;
        }
        return Ok(false);
    };

    if sip::is_request(frame, "NOTIFY")
        && sip::header_value(frame, "Event").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|event| event.trim().eq_ignore_ascii_case("refer"))
        })
    {
        let parsed = parse_refer_notify(frame);
        let accepted = parsed.as_ref().is_ok_and(|notification| {
            session
                .voice_calls
                .get_mut(&trunk_call_id)
                .is_some_and(|call| {
                    call.transfer
                        .as_mut()
                        .is_some_and(|transfer| transfer.on_notify(notification).is_ok())
                })
        });
        let (status, reason) = if accepted {
            (200, "OK")
        } else if parsed.is_err() {
            (400, "Bad Request")
        } else {
            (481, "Call/Transaction Does Not Exist")
        };
        let response = sip::build_response(frame, status, reason, None, None, None);
        session
            .channel
            .send_sip(&response)
            .await
            .map_err(map_channel_error)?;
        if accepted {
            live.operator.send_event(OperatorEvent::TransferNotify {
                call_id: trunk_call_id,
                notification: parsed.expect("accepted REFER notification must be parsed"),
            });
        }
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        return Ok(true);
    }

    if sip::is_request(frame, "INVITE") {
        let Some(trunk_local_ip) = live.operator.trunk_local_ip() else {
            send_incoming_rejection(session, runtime, frame, 480).await?;
            return Ok(true);
        };
        if !live.operator.is_available() {
            send_incoming_rejection(session, runtime, frame, 480).await?;
            return Ok(true);
        }
        if session.voice_calls.get(&trunk_call_id).is_some_and(|call| {
            call.pending_operator_reinvite.is_some() || call.pending_asterisk_reinvite
        }) {
            send_incoming_rejection(session, runtime, frame, 491).await?;
            return Ok(true);
        }
        let operator_audio = parse_audio_sdp(sip::sip_body(frame)).map_err(|error| {
            VolteError::with_detail("volte_voice_sdp_invalid", error.to_string())
        })?;
        let operator_remote = media_socket_addr(&operator_audio)?;
        let operator_video = parse_video_sdp(sip::sip_body(frame))
            .ok()
            .and_then(|description| {
                media_endpoint_for_video(&operator_audio, &description)
                    .ok()
                    .map(|endpoint| VideoOffer {
                        description,
                        endpoint,
                    })
            });
        if operator_video.is_some() && !live.operator.video_enabled() {
            send_incoming_rejection(session, runtime, frame, 488).await?;
            return Ok(true);
        }
        let pending =
            PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
                .await
                .map_err(|error| {
                    VolteError::with_detail("volte_rtp_bind_failed", error.to_string())
                })?;
        let operator_local = pending.operator_local_addr().map_err(|error| {
            VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
        })?;
        let internal_local = pending.internal_local_addr().map_err(|error| {
            VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
        })?;
        let (video_relay, operator_video_local, internal_video_local) = if operator_video.is_some()
        {
            let relay =
                PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
                    .await
                    .map_err(|error| {
                        VolteError::with_detail("vilte_rtp_bind_failed", error.to_string())
                    })?;
            let operator_local = relay.operator_local_addr().map_err(|error| {
                VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
            })?;
            let internal_local = relay.internal_local_addr().map_err(|error| {
                VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
            })?;
            (Some(relay), Some(operator_local), Some(internal_local))
        } else {
            (None, None, None)
        };
        let operator_dtmf = parse_rtp_telephone_event(sip::sip_body(frame));
        let offer = MediaOffer {
            audio: operator_audio,
            audio_endpoint: operator_remote,
            video: operator_video,
            dtmf: DtmfCapabilities {
                preferred: if operator_dtmf.is_some() {
                    DtmfSource::RtpEvent
                } else {
                    DtmfSource::SipInfo
                },
                rtp_event: operator_dtmf,
                sip_info: true,
            },
        };
        let trunk_offer = relay_media_sdp(&offer, internal_local, internal_video_local);
        let call = session
            .voice_calls
            .get_mut(&trunk_call_id)
            .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
        call.pending_operator_reinvite = Some(frame.to_vec());
        call.stage_media_update(
            offer,
            pending,
            operator_local,
            internal_local,
            video_relay,
            operator_video_local,
            internal_video_local,
        );
        call.renegotiation_deadline = Some(Instant::now() + REINVITE_TIMEOUT);
        let trying = sip::build_response(
            frame,
            100,
            "Trying",
            Some(&call.dialog.local_tag),
            None,
            None,
        );
        session
            .channel
            .send_sip(&trying)
            .await
            .map_err(map_channel_error)?;
        live.operator.send_event(OperatorEvent::Renegotiate {
            call_id: trunk_call_id,
            body: trunk_offer.into_bytes(),
        });
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        return Ok(true);
    }

    if sip::is_request(frame, "CANCEL") {
        if session
            .voice_calls
            .get(&trunk_call_id)
            .is_some_and(|call| call.operator_answered)
        {
            let completed = sip::build_response(
                frame,
                481,
                "Call/Transaction Does Not Exist",
                None,
                None,
                None,
            );
            session
                .channel
                .send_sip(&completed)
                .await
                .map_err(map_channel_error)?;
            runtime.update(|state| state.last_tx_at = Some(now())).await;
            return Ok(true);
        }
        let call = session
            .voice_calls
            .remove(&trunk_call_id)
            .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
        if call.direction != LiveVoiceDirection::MobileTerminated {
            return Ok(false);
        }
        let ok = sip::build_response(frame, 200, "OK", None, None, None);
        session
            .channel
            .send_sip(&ok)
            .await
            .map_err(map_channel_error)?;
        if let Some(invite) = call.initial_invite.as_deref() {
            let terminated = sip::build_response(
                invite,
                487,
                "Request Terminated",
                Some(&call.dialog.local_tag),
                None,
                None,
            );
            session
                .channel
                .send_sip(&terminated)
                .await
                .map_err(map_channel_error)?;
        }
        live.operator.send_event(OperatorEvent::Cancelled {
            call_id: trunk_call_id,
        });
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        return Ok(true);
    }

    if sip::is_request(frame, "BYE") {
        let response = sip::build_response(frame, 200, "OK", None, None, None);
        session
            .channel
            .send_sip(&response)
            .await
            .map_err(map_channel_error)?;
        session.voice_calls.remove(&trunk_call_id);
        live.operator.send_event(OperatorEvent::Ended {
            call_id: trunk_call_id,
        });
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        return Ok(true);
    }
    if sip::is_request(frame, "INFO") {
        let response = sip::build_response(frame, 200, "OK", None, None, None);
        session
            .channel
            .send_sip(&response)
            .await
            .map_err(map_channel_error)?;
        if let Some(signal) = crate::services::trunk::bridge::parse_operator_dtmf_info(frame) {
            live.operator.send_event(OperatorEvent::Dtmf {
                call_id: trunk_call_id,
                signal,
            });
        }
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        return Ok(true);
    }
    if frame.starts_with(b"SIP/2.0 ") {
        let status = sip::parse_status(frame)?;
        let method = sip::header_value(frame, "CSeq")
            .and_then(|value| value.split_whitespace().nth(1).map(str::to_string));
        if method
            .as_deref()
            .is_some_and(|method| method.eq_ignore_ascii_case("REFER"))
        {
            let call = session
                .voice_calls
                .get_mut(&trunk_call_id)
                .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
            let transfer = call
                .transfer
                .as_mut()
                .ok_or_else(|| VolteError::new("volte_transfer_not_pending"))?;
            transfer.on_refer_response(status).map_err(|error| {
                VolteError::with_detail("volte_transfer_response_invalid", error.to_string())
            })?;
            if status >= 200 {
                call.transfer_deadline = None;
            }
            live.operator.send_event(OperatorEvent::TransferResponse {
                call_id: trunk_call_id,
                status,
            });
            return Ok(true);
        }
        if method.as_deref() != Some("INVITE") {
            return Ok(true);
        }
        let is_asterisk_reinvite = session
            .voice_calls
            .get(&trunk_call_id)
            .is_some_and(|call| call.pending_asterisk_reinvite);
        if (100..200).contains(&status) {
            if is_asterisk_reinvite {
                live.operator.send_event(OperatorEvent::Provisional {
                    call_id: trunk_call_id,
                    status,
                    body: None,
                });
                return Ok(true);
            }
            let identity = session.identity.clone();
            let route = session.channel.route();
            let delayed_ip_connect =
                live.operator.ip_connect_mode() == TrunkIpConnectMode::FirstRtp;
            let (body, prack, first_operator_rtp) = {
                let call = session
                    .voice_calls
                    .get_mut(&trunk_call_id)
                    .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
                if let Some(tag) = response_to_tag(frame) {
                    call.dialog.set_remote_tag(tag);
                }
                let body = if sip::sip_body(frame).is_empty() {
                    None
                } else {
                    let answer = prepare_operator_media(call, sip::sip_body(frame))?;
                    // Retain the early answer so a final 200 OK without SDP can
                    // reuse it instead of being rejected as unusable media.
                    call.early_answer = Some(answer.clone());
                    Some(answer)
                };
                let first_operator_rtp = delayed_ip_connect
                    .then(|| arm_first_rtp_ip_answer(call))
                    .flatten();
                let reliable = sip::header_value(frame, "Require")
                    .is_some_and(|value| value.split(',').any(|item| item.trim() == "100rel"));
                let prack = if reliable {
                    let rseq = sip::header_value(frame, "RSeq")
                        .and_then(|value| value.trim().parse::<u32>().ok())
                        .ok_or_else(|| VolteError::new("volte_voice_rseq_missing"))?;
                    let cseq = call.next_cseq;
                    call.next_cseq = call.next_cseq.saturating_add(1);
                    Some(sip::build_prack(
                        &identity,
                        &route,
                        session.registration.service_route.as_deref(),
                        &call.dialog,
                        &call.callee_uri,
                        cseq,
                        rseq,
                        call.dialog.cseq,
                    ))
                } else {
                    None
                };
                (body, prack, first_operator_rtp)
            };
            if let Some(prack) = prack {
                session
                    .channel
                    .send_sip(&prack)
                    .await
                    .map_err(map_channel_error)?;
                runtime.update(|state| state.last_tx_at = Some(now())).await;
            }
            live.operator.send_event(OperatorEvent::Provisional {
                call_id: trunk_call_id.clone(),
                status,
                body: body.clone().map(String::into_bytes),
            });
            if let (Some(answer), Some(first_operator_rtp)) = (body, first_operator_rtp) {
                spawn_first_rtp_ip_answer(
                    live.operator.clone(),
                    trunk_call_id,
                    answer.into_bytes(),
                    first_operator_rtp,
                );
            }
            return Ok(true);
        }
        if (200..300).contains(&status) {
            let identity = session.identity.clone();
            let route = session.channel.route();
            let immediate_ip_connect =
                live.operator.ip_connect_mode() == TrunkIpConnectMode::GsmAnswer;
            let (ack, answer, first_operator_rtp) = {
                let call = session
                    .voice_calls
                    .get_mut(&trunk_call_id)
                    .ok_or_else(|| VolteError::new("volte_voice_call_unknown"))?;
                let tag = response_to_tag(frame)
                    .ok_or_else(|| VolteError::new("volte_voice_remote_tag_missing"))?;
                call.dialog.set_remote_tag(tag);
                let answer = prepare_final_operator_media(call, sip::sip_body(frame));
                let first_operator_rtp =
                    if !is_asterisk_reinvite && !immediate_ip_connect && answer.is_ok() {
                        arm_first_rtp_ip_answer(call)
                    } else {
                        None
                    };
                call.pending_asterisk_reinvite = false;
                call.operator_answered = true;
                if answer.is_ok() {
                    call.commit_media_update();
                }
                // The early answer only applies to the INVITE it was negotiated
                // for; drop it so a later re-INVITE cannot reuse a stale SDP.
                call.early_answer = None;
                call.renegotiation_deadline = None;
                let ack = sip::build_ack(
                    &identity,
                    &route,
                    session.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.callee_uri,
                );
                (ack, answer, first_operator_rtp)
            };
            session
                .channel
                .send_sip(&ack)
                .await
                .map_err(map_channel_error)?;
            runtime.update(|state| state.last_tx_at = Some(now())).await;
            match answer {
                Ok(answer) if immediate_ip_connect || is_asterisk_reinvite => {
                    live.operator.send_event(OperatorEvent::Answered {
                        call_id: trunk_call_id,
                        body: answer.into_bytes(),
                    });
                }
                Ok(answer) => {
                    if let Some(first_operator_rtp) = first_operator_rtp {
                        spawn_first_rtp_ip_answer(
                            live.operator.clone(),
                            trunk_call_id,
                            answer.into_bytes(),
                            first_operator_rtp,
                        );
                    }
                }
                Err(error) => {
                    let bye = session.voice_calls.get(&trunk_call_id).map(|call| {
                        sip::build_bye(
                            &identity,
                            &route,
                            session.registration.service_route.as_deref(),
                            &call.dialog,
                            &call.callee_uri,
                            call.next_cseq,
                        )
                    });
                    if let Some(bye) = bye {
                        session
                            .channel
                            .send_sip(&bye)
                            .await
                            .map_err(map_channel_error)?;
                    }
                    session.voice_calls.remove(&trunk_call_id);
                    live.operator.send_event(OperatorEvent::Rejected {
                        call_id: trunk_call_id,
                        status: 488,
                    });
                    tracing::warn!(error = %error, "Rejected IMS answer with unusable media");
                }
            }
            return Ok(true);
        }
        if is_asterisk_reinvite {
            if let Some(call) = session.voice_calls.get_mut(&trunk_call_id) {
                call.pending_asterisk_reinvite = false;
                call.rollback_media_update();
                call.renegotiation_deadline = None;
            }
        } else {
            session.voice_calls.remove(&trunk_call_id);
        }
        live.operator.send_event(OperatorEvent::Rejected {
            call_id: trunk_call_id,
            status,
        });
        return Ok(true);
    }
    Ok(false)
}

async fn begin_incoming_operator_call(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    session: &mut VolteLiveSession,
    frame: &[u8],
    ims_call_id: String,
) -> Result<bool, VolteError> {
    let Some(trunk_local_ip) = live.operator.trunk_local_ip() else {
        send_incoming_rejection(session, runtime, frame, 480).await?;
        return Ok(true);
    };
    if !live.operator.is_available() {
        send_incoming_rejection(session, runtime, frame, 480).await?;
        return Ok(true);
    }
    if session.voice_calls.len() >= MAX_CONCURRENT_CALLS {
        send_incoming_rejection(session, runtime, frame, 486).await?;
        return Ok(true);
    }
    let operator_audio = match parse_audio_sdp(sip::sip_body(frame)) {
        Ok(audio) => audio,
        Err(_) => {
            send_incoming_rejection(session, runtime, frame, 488).await?;
            return Ok(true);
        }
    };
    let operator_remote = match media_socket_addr(&operator_audio) {
        Ok(remote) => remote,
        Err(_) => {
            send_incoming_rejection(session, runtime, frame, 488).await?;
            return Ok(true);
        }
    };
    let operator_video = parse_video_sdp(sip::sip_body(frame))
        .ok()
        .and_then(|description| {
            media_endpoint_for_video(&operator_audio, &description)
                .ok()
                .map(|endpoint| VideoOffer {
                    description,
                    endpoint,
                })
        });
    if operator_video.is_some() && !live.operator.video_enabled() {
        send_incoming_rejection(session, runtime, frame, 488).await?;
        return Ok(true);
    }
    let Some(from_uri) = sip::sip_header_uri(frame, "From") else {
        send_incoming_rejection(session, runtime, frame, 400).await?;
        return Ok(true);
    };
    let caller = crate::connectivity::core::supplementary::resolve_caller_identity(frame)
        .uri
        .as_deref()
        .map(normalize_incoming_caller)
        .unwrap_or_else(|| "sip:anonymous@anonymous.invalid".to_string());
    let remote_target = sip::sip_header_uri(frame, "Contact").unwrap_or(from_uri);
    let Some(remote_tag) =
        sip::header_value(frame, "From").and_then(|value| sip_parameter(&value, "tag"))
    else {
        send_incoming_rejection(session, runtime, frame, 400).await?;
        return Ok(true);
    };
    let Some(invite_cseq) = sip::header_value(frame, "CSeq")
        .and_then(|value| value.split_whitespace().next()?.parse::<u32>().ok())
    else {
        send_incoming_rejection(session, runtime, frame, 400).await?;
        return Ok(true);
    };
    let relay = match PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
        .await
    {
        Ok(relay) => relay,
        Err(error) => {
            tracing::warn!(error = %error, "Unable to allocate MT RTP relay");
            send_incoming_rejection(session, runtime, frame, 500).await?;
            return Ok(true);
        }
    };
    let operator_local = relay.operator_local_addr().map_err(|error| {
        VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
    })?;
    let internal_local = relay.internal_local_addr().map_err(|error| {
        VolteError::with_detail("volte_rtp_local_addr_failed", error.to_string())
    })?;
    let (video_relay, operator_video_local, internal_video_local) = if operator_video.is_some() {
        let relay = PendingRtpRelay::bind(session.channel.route().local_addr.ip(), trunk_local_ip)
            .await
            .map_err(|error| VolteError::with_detail("vilte_rtp_bind_failed", error.to_string()))?;
        let operator_local = relay.operator_local_addr().map_err(|error| {
            VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
        })?;
        let internal_local = relay.internal_local_addr().map_err(|error| {
            VolteError::with_detail("vilte_rtp_local_addr_failed", error.to_string())
        })?;
        (Some(relay), Some(operator_local), Some(internal_local))
    } else {
        (None, None, None)
    };
    let operator_dtmf = parse_rtp_telephone_event(sip::sip_body(frame));
    let media_offer = MediaOffer {
        audio: operator_audio.clone(),
        audio_endpoint: operator_remote,
        video: operator_video,
        dtmf: DtmfCapabilities {
            preferred: if operator_dtmf.is_some() {
                DtmfSource::RtpEvent
            } else {
                DtmfSource::SipInfo
            },
            rtp_event: operator_dtmf.clone(),
            sip_info: true,
        },
    };
    let trunk_offer = relay_media_sdp(&media_offer, internal_local, internal_video_local);
    let dialog = sip::DialogIds {
        call_id: ims_call_id.clone(),
        local_tag: sip::hex_token(8),
        remote_tag: Some(remote_tag),
        cseq: invite_cseq,
    };
    let trying = sip::build_response(frame, 100, "Trying", Some(&dialog.local_tag), None, None);
    session
        .channel
        .send_sip(&trying)
        .await
        .map_err(map_channel_error)?;
    let operator_answered = live.operator.incoming_mode() == TrunkIncomingMode::BoundImmediate;
    if operator_answered {
        let contact = ims_contact(&session.identity, &session.channel.route());
        let answer = relay_media_sdp(&media_offer, operator_local, operator_video_local);
        let accepted = sip::build_response(
            frame,
            200,
            "OK",
            Some(&dialog.local_tag),
            Some(&contact),
            Some(answer.as_bytes()),
        );
        session
            .channel
            .send_sip(&accepted)
            .await
            .map_err(map_channel_error)?;
    }
    session.voice_calls.insert(
        ims_call_id.clone(),
        LiveVoiceCall {
            direction: LiveVoiceDirection::MobileTerminated,
            dialog,
            callee_uri: remote_target,
            invite_branch: String::new(),
            initial_invite: Some(frame.to_vec()),
            internal_offer: media_offer,
            operator_local,
            internal_local,
            pending_relay: Some(relay),
            active_relay: None,
            ip_answer_wait_armed: false,
            operator_answered,
            next_cseq: 1,
            media_metrics: Some(live.operator.media_metrics()),
            pending_operator_reinvite: None,
            pending_asterisk_reinvite: false,
            pending_video_relay: video_relay,
            active_video_relay: None,
            operator_video_local,
            internal_video_local,
            pending_media_rollback: None,
            renegotiation_deadline: None,
            early_answer: None,
            transfer: None,
            transfer_deadline: None,
        },
    );
    live.operator.send_event(OperatorEvent::Incoming {
        call_id: ims_call_id,
        caller,
        body: trunk_offer.into_bytes(),
    });
    runtime.update(|state| state.last_tx_at = Some(now())).await;
    Ok(true)
}

async fn send_incoming_rejection(
    session: &mut VolteLiveSession,
    runtime: &Arc<VolteRuntime>,
    frame: &[u8],
    status: u16,
) -> Result<(), VolteError> {
    let response = sip::build_response(frame, status, ims_reason(status), None, None, None);
    session
        .channel
        .send_sip(&response)
        .await
        .map_err(map_channel_error)?;
    runtime.update(|state| state.last_tx_at = Some(now())).await;
    Ok(())
}

fn sip_parameter(value: &str, expected: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.trim().split_once('=')?;
        name.eq_ignore_ascii_case(expected)
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

fn normalize_incoming_caller(caller: &str) -> String {
    if let Some(number) = caller.strip_prefix("tel:") {
        format!("sip:{number}@simadmin")
    } else if let Some(uri) = caller.strip_prefix("sips:") {
        format!("sip:{uri}")
    } else {
        caller.to_string()
    }
}

fn prepare_operator_media(call: &mut LiveVoiceCall, body: &[u8]) -> Result<String, VolteError> {
    let operator_audio = parse_audio_sdp(body)
        .map_err(|error| VolteError::with_detail("volte_voice_sdp_invalid", error.to_string()))?;
    let operator_remote = media_socket_addr(&operator_audio)?;
    let mut internal_answer = operator_audio.clone();
    internal_answer.direction = operator_audio.direction.for_peer();
    internal_answer.codecs = operator_audio
        .codecs
        .iter()
        .filter_map(|operator_codec| {
            call.internal_offer
                .audio
                .find_matching_codec(operator_codec)
                .cloned()
        })
        .collect();
    if internal_answer.codecs.is_empty() {
        return Err(VolteError::new("volte_voice_no_common_codec"));
    }
    let operator_dtmf = parse_rtp_telephone_event(body);
    let internal_dtmf = call.internal_offer.dtmf.rtp_event.as_ref();
    let mut mappings = operator_audio
        .codecs
        .iter()
        .filter_map(|operator_codec| {
            let internal = call
                .internal_offer
                .audio
                .find_matching_codec(operator_codec)?;
            (operator_codec.payload_type != internal.payload_type).then_some(PayloadTypeMapping {
                operator: operator_codec.payload_type,
                internal: internal.payload_type,
            })
        })
        .collect::<Vec<_>>();
    if let (Some(operator), Some(internal)) = (operator_dtmf.as_ref(), internal_dtmf) {
        if operator.payload_type != internal.payload_type {
            mappings.push(PayloadTypeMapping {
                operator: operator.payload_type,
                internal: internal.payload_type,
            });
        }
    }
    if call.active_relay.is_none() || call.pending_relay.is_some() {
        let pending = call
            .pending_relay
            .take()
            .ok_or_else(|| VolteError::new("volte_rtp_relay_missing"))?;
        let policy = MediaRelayPolicy::from_directions(
            operator_audio.direction,
            call.internal_offer.audio.direction,
        );
        call.active_relay = Some(pending.activate_with_metrics_and_policy(
            operator_remote,
            call.internal_offer.audio_endpoint,
            mappings,
            policy,
            call.media_metrics.clone(),
        ));
    }
    let mut answer = relay_audio_sdp(&internal_answer, internal_dtmf, call.internal_local);
    if let (Some(internal_video), Ok(operator_video), Some(_), Some(internal_local)) = (
        call.internal_offer.video.as_ref(),
        parse_video_sdp(body),
        call.operator_video_local,
        call.internal_video_local,
    ) {
        negotiate_video(&internal_video.description, &operator_video).map_err(|error| {
            VolteError::with_detail("vilte_video_negotiation_failed", error.to_string())
        })?;
        let operator_remote = media_endpoint_for_video(&operator_audio, &operator_video)?;
        if call.active_video_relay.is_none() || call.pending_video_relay.is_some() {
            let pending = call
                .pending_video_relay
                .take()
                .ok_or_else(|| VolteError::new("vilte_rtp_relay_missing"))?;
            let mappings = (operator_video.payload_type != internal_video.description.payload_type)
                .then_some(PayloadTypeMapping {
                    operator: operator_video.payload_type,
                    internal: internal_video.description.payload_type,
                });
            call.active_video_relay = Some(pending.activate_with_metrics(
                operator_remote,
                internal_video.endpoint,
                mappings,
                call.media_metrics.clone(),
            ));
        }
        let mut trunk_video = operator_video;
        trunk_video.direction = trunk_video.direction.for_peer();
        trunk_video.media_port = internal_local.port();
        trunk_video.connection_addr = Some(internal_local.ip().to_string());
        trunk_video.addr_type = Some(if internal_local.is_ipv4() {
            SdpAddrType::Ip4
        } else {
            SdpAddrType::Ip6
        });
        answer.push_str(&trunk_video.media_lines());
    } else {
        call.pending_video_relay = None;
        call.active_video_relay = None;
    }
    Ok(answer)
}

fn prepare_final_operator_media(
    call: &mut LiveVoiceCall,
    body: &[u8],
) -> Result<String, VolteError> {
    if body.is_empty() {
        return call.early_answer.clone().ok_or_else(|| {
            VolteError::with_detail("volte_voice_sdp_invalid", "voice_sdp_empty".to_string())
        });
    }
    prepare_operator_media(call, body)
}

fn arm_first_rtp_ip_answer(call: &mut LiveVoiceCall) -> Option<tokio::sync::watch::Receiver<bool>> {
    if call.ip_answer_wait_armed {
        return None;
    }
    let first_operator_rtp = call.active_relay.as_ref()?.subscribe_first_operator_rtp();
    call.ip_answer_wait_armed = true;
    Some(first_operator_rtp)
}

fn spawn_first_rtp_ip_answer(
    operator: OperatorLink,
    call_id: String,
    body: Vec<u8>,
    mut first_operator_rtp: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        while !*first_operator_rtp.borrow() {
            if first_operator_rtp.changed().await.is_err() {
                return;
            }
        }
        operator.send_event(OperatorEvent::Answered { call_id, body });
    });
}

/// Translate Asterisk's answer for an MT call back into the payload numbers
/// and relay address advertised on the operator IMS dialog.
fn prepare_incoming_media(call: &mut LiveVoiceCall, body: &[u8]) -> Result<String, VolteError> {
    let internal_audio = parse_audio_sdp(body)
        .map_err(|error| VolteError::with_detail("volte_voice_sdp_invalid", error.to_string()))?;
    let internal_remote = media_socket_addr(&internal_audio)?;
    let mut operator_answer = call.internal_offer.audio.clone();
    operator_answer.direction = internal_audio.direction.for_peer();
    operator_answer.codecs = call
        .internal_offer
        .audio
        .codecs
        .iter()
        .filter(|operator| internal_audio.find_matching_codec(operator).is_some())
        .cloned()
        .collect();
    if operator_answer.codecs.is_empty() {
        return Err(VolteError::new("volte_voice_no_common_codec"));
    }
    let operator_dtmf = call.internal_offer.dtmf.rtp_event.as_ref();
    let internal_dtmf = parse_rtp_telephone_event(body);
    let mut mappings = operator_answer
        .codecs
        .iter()
        .filter_map(|operator| {
            let internal = internal_audio.find_matching_codec(operator)?;
            (operator.payload_type != internal.payload_type).then_some(PayloadTypeMapping {
                operator: operator.payload_type,
                internal: internal.payload_type,
            })
        })
        .collect::<Vec<_>>();
    if let (Some(operator), Some(internal)) = (operator_dtmf, internal_dtmf.as_ref()) {
        if operator.payload_type != internal.payload_type {
            mappings.push(PayloadTypeMapping {
                operator: operator.payload_type,
                internal: internal.payload_type,
            });
        }
    }
    if call.active_relay.is_none() || call.pending_relay.is_some() {
        let pending = call
            .pending_relay
            .take()
            .ok_or_else(|| VolteError::new("volte_rtp_relay_missing"))?;
        let policy = MediaRelayPolicy::from_directions(
            call.internal_offer.audio.direction,
            internal_audio.direction,
        );
        call.active_relay = Some(pending.activate_with_metrics_and_policy(
            call.internal_offer.audio_endpoint,
            internal_remote,
            mappings,
            policy,
            call.media_metrics.clone(),
        ));
    }
    let mut answer = relay_audio_sdp(&operator_answer, operator_dtmf, call.operator_local);
    if let (Some(operator_video), Ok(internal_video), Some(operator_local)) = (
        call.internal_offer.video.as_ref(),
        parse_video_sdp(body),
        call.operator_video_local,
    ) {
        negotiate_video(&operator_video.description, &internal_video).map_err(|error| {
            VolteError::with_detail("vilte_video_negotiation_failed", error.to_string())
        })?;
        let internal_remote = media_endpoint_for_video(&internal_audio, &internal_video)?;
        if call.active_video_relay.is_none() || call.pending_video_relay.is_some() {
            let pending = call
                .pending_video_relay
                .take()
                .ok_or_else(|| VolteError::new("vilte_rtp_relay_missing"))?;
            let mappings = (operator_video.description.payload_type != internal_video.payload_type)
                .then_some(PayloadTypeMapping {
                    operator: operator_video.description.payload_type,
                    internal: internal_video.payload_type,
                });
            call.active_video_relay = Some(pending.activate_with_metrics(
                operator_video.endpoint,
                internal_remote,
                mappings,
                call.media_metrics.clone(),
            ));
        }
        let mut ims_video = operator_video.description.clone();
        ims_video.direction = internal_video.direction.for_peer();
        ims_video.media_port = operator_local.port();
        ims_video.connection_addr = Some(operator_local.ip().to_string());
        ims_video.addr_type = Some(if operator_local.is_ipv4() {
            SdpAddrType::Ip4
        } else {
            SdpAddrType::Ip6
        });
        answer.push_str(&ims_video.media_lines());
    } else {
        call.pending_video_relay = None;
        call.active_video_relay = None;
    }
    Ok(answer)
}

fn media_endpoint_for_video(
    audio: &SdpAudioDescription,
    video: &VideoMediaDescription,
) -> Result<SocketAddr, VolteError> {
    let ip = video
        .connection_addr
        .as_deref()
        .unwrap_or(&audio.connection_addr)
        .parse::<IpAddr>()
        .map_err(|_| VolteError::new("vilte_video_address_invalid"))?;
    if video.media_port == 0 {
        return Err(VolteError::new("vilte_video_port_invalid"));
    }
    Ok(SocketAddr::new(ip, video.media_port))
}

fn ims_contact(identity: &ImsIdentity, route: &ImsRoute) -> String {
    format!(
        "sip:{}@{}:{};transport={}",
        identity.contact_user,
        sip::sip_host(route.local_addr.ip()),
        route.local_addr.port(),
        route.transport.as_param(),
    )
}

fn ims_reason(status: u16) -> &'static str {
    match status {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        480 => "Temporarily Unavailable",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        491 => "Request Pending",
        500 => "Server Internal Error",
        603 => "Decline",
        _ => "Call Failed",
    }
}

fn media_socket_addr(audio: &SdpAudioDescription) -> Result<SocketAddr, VolteError> {
    let ip = audio
        .connection_addr
        .parse::<IpAddr>()
        .map_err(|_| VolteError::new("volte_voice_media_address_invalid"))?;
    if audio.media_port == 0 {
        return Err(VolteError::new("volte_voice_media_port_invalid"));
    }
    Ok(SocketAddr::new(ip, audio.media_port))
}

fn response_to_tag(frame: &[u8]) -> Option<String> {
    sip::header_value(frame, "To")?
        .split(';')
        .find_map(|parameter| parameter.trim().strip_prefix("tag=").map(ToOwned::to_owned))
}

async fn handle_live_frame(
    context: LiveFrameContext<'_>,
    reassembler: &mut MtReassembler,
    frame: &[u8],
) -> Result<(), VolteError> {
    let LiveFrameContext {
        live,
        line_id,
        runtime,
        database,
        notification_sender,
        dedupe_enabled,
    } = context;
    if handle_volte_mwi_frame(live, runtime, frame).await? {
        return Ok(());
    }
    if handle_operator_sip_frame(live, runtime, frame).await? {
        return Ok(());
    }
    if !sip::is_request(frame, "MESSAGE") {
        if let Ok(sip_status) = sip::parse_status(frame) {
            tracing::debug!(sip_status, "VoLTE protected SIP response received");
            return Ok(());
        }
        let (status, reason) = if sip::is_request(frame, "INVITE") {
            (480, "Temporarily Unavailable")
        } else {
            (200, "OK")
        };
        let response = sip::build_response(frame, status, reason, None, None, None);
        send_live_frame(live, runtime, &response).await?;
        return Ok(());
    }

    // Complete the SIP transaction before parsing/storing the RP-DATA.
    let response = sip::build_response(frame, 200, "OK", None, None, None);
    send_live_frame(live, runtime, &response).await?;

    let deliver = crate::connectivity::core::sms_codec::parse_mt_rp_data(sip::sip_body(frame))
        .map_err(|_| VolteError::new("volte_mt_rp_data_invalid"))?;
    let rp_ack_body =
        crate::connectivity::core::sms_codec::build_network_rp_ack(deliver.rp_message_reference);
    let rp_ack = {
        let sessions = live.session.lock().await;
        let session = sessions
            .as_ref()
            .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
        sip::build_rp_ack(
            &session.identity,
            &session.channel.route(),
            session.registration.service_route.as_deref(),
            frame,
            &rp_ack_body,
            &session.identity.public_uri,
            session.channel.security_verify(),
        )
    };
    send_live_frame(live, runtime, &rp_ack).await?;
    tracing::info!(
        originator_len = deliver.originator.len(),
        body_bytes = sip::sip_body(frame).len(),
        "VoLTE MT SMS received and RP-ACK submitted"
    );

    match reassembler.ingest_deliver(deliver) {
        MtIngest::Complete(message) => {
            if dedupe_enabled {
                let fingerprint = crate::services::orchestrator::message_fingerprint(
                    &crate::services::orchestrator::MessageFingerprintInput {
                        service_center_timestamp: &message.service_center_timestamp,
                        originator: &message.originator,
                        text: &message.text,
                        segment_reference: None,
                        segment_sequence: 1,
                        segment_total: 1,
                    },
                );
                let claimed = database
                    .claim_sms_dedup(line_id, &fingerprint, TRANSPORT_TAG)
                    .map_err(|error| {
                        VolteError::with_detail("volte_sms_db_failed", error.to_string())
                    })?;
                if !claimed {
                    runtime.update(|state| state.duplicate_count += 1).await;
                    return Ok(());
                }
            }
            if database
                .sms_exists_by_pdu_for_line(line_id, &message.dedup_marker)
                .map_err(|error| {
                    VolteError::with_detail("volte_sms_db_failed", error.to_string())
                })?
            {
                runtime.update(|state| state.duplicate_count += 1).await;
                return Ok(());
            }
            let timestamp = if message.service_center_timestamp.trim().is_empty() {
                crate::platform::db::utc_sms_now_string()
            } else {
                message.service_center_timestamp.clone()
            };
            let id = database
                .insert_sms_at_with_transport_for_line(
                    "incoming",
                    &message.originator,
                    &message.text,
                    &timestamp,
                    "received",
                    Some(&message.dedup_marker),
                    TRANSPORT_TAG,
                    Some(line_id),
                )
                .map_err(|error| {
                    VolteError::with_detail("volte_sms_db_failed", error.to_string())
                })?;
            runtime.update(|state| state.received_count += 1).await;
            let sms = SmsMessage {
                id,
                direction: "incoming".to_string(),
                phone_number: message.originator,
                content: message.text,
                timestamp,
                status: "received".to_string(),
                pdu: Some(message.dedup_marker),
                transport: TRANSPORT_TAG.to_string(),
                line_id: Some(line_id.to_string()),
            };
            let notification_sender = Arc::clone(notification_sender);
            tokio::spawn(async move {
                let _ = notification_sender.forward_sms(&sms).await;
            });
            tracing::info!(segment_total = message.segment_total, "Stored VoLTE MT SMS");
        }
        MtIngest::Buffered {
            reference,
            have,
            total,
        } => {
            tracing::debug!(
                reference,
                have,
                total,
                "Buffered VoLTE MT multipart segment"
            );
        }
        MtIngest::ParseError => return Err(VolteError::new("volte_mt_rp_data_invalid")),
    }
    Ok(())
}

async fn handle_volte_mwi_frame(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    frame: &[u8],
) -> Result<bool, VolteError> {
    let active_call_id = {
        let sessions = live.session.lock().await;
        sessions.as_ref().and_then(|session| {
            session
                .mwi_subscription
                .as_ref()
                .map(|subscription| subscription.ids.call_id.clone())
        })
    };
    match classify_mwi_frame(frame, active_call_id.as_deref()) {
        MwiIncomingFrame::Notify {
            response_status,
            summary,
        } => {
            let reason = if response_status == 200 {
                "OK"
            } else {
                "Call/Transaction Does Not Exist"
            };
            let response = sip::build_response(frame, response_status, reason, None, None, None);
            send_live_frame(live, runtime, &response).await?;
            if let (Some(supplementary), Some(summary)) = (live.supplementary_runtime(), summary) {
                match summary {
                    Ok(summary) => {
                        supplementary
                            .update_message_waiting(ImsRegistrationAccess::Volte, summary)
                            .await;
                    }
                    Err(error) => {
                        supplementary
                            .fail_mwi_subscription(ImsRegistrationAccess::Volte, error.code())
                            .await;
                    }
                }
            }
            Ok(true)
        }
        MwiIncomingFrame::SubscribeResponse { status, to_tag } => {
            let Some(supplementary) = live.supplementary_runtime() else {
                return Ok(true);
            };
            match status {
                Ok(200..=299) => {
                    if let Some(tag) = to_tag {
                        let mut sessions = live.session.lock().await;
                        if let Some(subscription) = sessions
                            .as_mut()
                            .and_then(|session| session.mwi_subscription.as_mut())
                        {
                            subscription.ids.to_tag = Some(tag);
                        }
                    }
                    supplementary
                        .mark_mwi_subscribed(ImsRegistrationAccess::Volte)
                        .await;
                }
                Ok(401 | 407) => {
                    if let Err(error) = retry_volte_mwi_subscription_with_aka(live, frame).await {
                        supplementary
                            .fail_mwi_subscription(ImsRegistrationAccess::Volte, error.code())
                            .await;
                    }
                }
                Ok(_) | Err(_) => {
                    supplementary
                        .fail_mwi_subscription(
                            ImsRegistrationAccess::Volte,
                            "mwi_subscribe_rejected",
                        )
                        .await;
                }
            }
            Ok(true)
        }
        MwiIncomingFrame::Other => Ok(false),
    }
}

async fn retry_volte_mwi_subscription_with_aka(
    live: &VolteLiveHandle,
    challenge_frame: &[u8],
) -> Result<(), VolteError> {
    let (device, aid, identity, route, registration, profile, security_verify) = {
        let sessions = live.session.lock().await;
        let session = sessions
            .as_ref()
            .ok_or_else(|| VolteError::new("mwi_subscription_missing"))?;
        let subscription = session
            .mwi_subscription
            .as_ref()
            .ok_or_else(|| VolteError::new("mwi_subscription_missing"))?;
        if subscription.authenticated {
            return Err(VolteError::new("mwi_subscribe_authentication_rejected"));
        }
        (
            session.device.clone(),
            session.aka_aid.clone(),
            session.identity.clone(),
            session.channel.route(),
            session.registration.clone(),
            session.profile,
            session.channel.security_verify().map(str::to_string),
        )
    };
    let challenge = parse_digest_challenge(challenge_frame)?;
    let aka_challenge = digest_aka::decode_aka_nonce(&challenge.nonce)?;
    let aka = tokio::task::spawn_blocking(move || {
        identity::run_usim_aka(
            QMI_PROXY_SOCKET,
            &device.qmi_device,
            device.uim_slot,
            &aid,
            &aka_challenge.rand,
            &aka_challenge.autn,
            2,
            Duration::from_secs(5),
            Duration::from_millis(300),
        )
    })
    .await
    .map_err(|_| VolteError::new(code::USIM_AKA_FAILED))??;
    let cnonce = sip::hex_token(8);
    let digest_uri = identity.public_uri.as_str();
    let authorization = if let Some(auts) = aka.auts.as_deref() {
        crate::connectivity::core::digest_aka::build_resync_authorization_header_with_digest(
            &challenge,
            &identity.private_user,
            digest_uri,
            auts,
            challenge.qop.as_ref().map(|_| cnonce.as_str()),
            challenge.qop.as_ref().map(|_| "00000001"),
        )
    } else {
        let response = digest_aka::compute_aka_response(
            &identity.private_user,
            &challenge.realm,
            &aka,
            &challenge.algorithm,
            "SUBSCRIBE",
            digest_uri,
            &challenge.nonce,
            challenge.qop.as_deref(),
            &cnonce,
            "00000001",
        )?;
        digest_aka::build_authorization_header(
            &challenge,
            &identity.private_user,
            digest_uri,
            &response,
            &cnonce,
            "00000001",
        )
    };
    let (header_name, header_value) = authorization
        .split_once(':')
        .ok_or_else(|| VolteError::new("mwi_authorization_header_invalid"))?;

    let mut sessions = live.session.lock().await;
    let session = sessions
        .as_mut()
        .ok_or_else(|| VolteError::new("mwi_subscription_missing"))?;
    let subscription = session
        .mwi_subscription
        .as_mut()
        .ok_or_else(|| VolteError::new("mwi_subscription_missing"))?;
    if subscription.authenticated {
        return Err(VolteError::new("mwi_subscribe_authentication_rejected"));
    }
    subscription.ids.branch = sip::new_branch();
    subscription.ids.cseq = subscription.ids.cseq.saturating_add(1);
    let mut access_headers = vec![
        SipHeader::new("P-Access-Network-Info", sip::PANI_EUTRAN),
        SipHeader::new(header_name.trim(), header_value.trim()),
    ];
    if let Some(value) = security_verify {
        access_headers.push(SipHeader::new("Security-Verify", value));
    }
    let request = build_mwi_subscribe(
        &identity,
        &route,
        &registration,
        &subscription.ids,
        MWI_SUBSCRIBE_EXPIRES_SECONDS,
        profile.ims.user_agent,
        &access_headers,
    );
    session
        .channel
        .send_sip(&request)
        .await
        .map_err(map_channel_error)?;
    subscription.authenticated = true;
    Ok(())
}

async fn send_live_frame(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    frame: &[u8],
) -> Result<(), VolteError> {
    let mut sessions = live.session.lock().await;
    let session = sessions
        .as_mut()
        .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
    session
        .channel
        .send_sip(frame)
        .await
        .map_err(map_channel_error)?;
    runtime.update(|state| state.last_tx_at = Some(now())).await;
    Ok(())
}

pub async fn send_live_sms_for_line(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    recipient: &str,
    text: &str,
    service_center: &str,
) -> Result<VolteSmsSendResult, VolteError> {
    if !runtime.status().await.registered {
        return Err(VolteError::new("volte_runtime_not_registered"));
    }
    if service_center.trim().is_empty() {
        return Err(VolteError::new("volte_smsc_missing"));
    }
    let submissions = crate::connectivity::modems::ims::volte::sms::build_mo_submissions(
        recipient,
        text,
        service_center,
    )
    .map_err(|error| VolteError::with_detail("volte_sms_encode_failed", error.to_string()))?;
    let first = submissions
        .first()
        .ok_or_else(|| VolteError::new("volte_sms_encode_failed"))?;
    let message_id = first.message_id.clone();
    let trace_id = first.trace_id.clone();
    let part_count = submissions.len();
    let mut sip_statuses = Vec::with_capacity(part_count);

    for submission in submissions {
        let mut sessions = live.session.lock().await;
        let session = sessions
            .as_mut()
            .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
        let (service_center_uri, recipient_uri) =
            mo_sms_uris(recipient, service_center, &session.identity.home_domain)?;
        let frame = sip::build_sms_message(
            &session.identity,
            &session.channel.route(),
            session.registration.service_route.as_deref(),
            &service_center_uri,
            &recipient_uri,
            &submission.body,
            session.channel.security_verify(),
        );
        session
            .channel
            .send_sip(&frame)
            .await
            .map_err(map_channel_error)?;
        runtime.update(|state| state.last_tx_at = Some(now())).await;
        let response = session
            .channel
            .recv_sip(Duration::from_secs(10))
            .await
            .map_err(map_channel_error)?;
        let sip_status = sip::parse_status(&response)?;
        runtime.update(|state| state.last_rx_at = Some(now())).await;
        if !(200..300).contains(&sip_status) {
            tracing::warn!(
                sip_status,
                reason = ?sip::header_value(&response, "Reason"),
                warning = ?sip::header_value(&response, "Warning"),
                service_route_present = session.registration.service_route.is_some(),
                "VoLTE MO SMS SIP MESSAGE rejected"
            );
            return Err(VolteError::with_detail(
                "volte_sms_message_rejected",
                sip_status.to_string(),
            ));
        }
        sip_statuses.push(sip_status);
    }
    runtime.update(|state| state.sent_count += 1).await;
    Ok(VolteSmsSendResult {
        message_id,
        trace_id,
        part_count,
        sip_statuses,
    })
}

fn register_service_route(response: &[u8]) -> Option<String> {
    RegisterArtifacts::parse(response).service_route
}

/// Return the network-selected default public user identity from REGISTER.
/// P-Associated-URI is an ordered list (possibly repeated across header lines),
/// so the first supported URI is the default identity for later requests.
fn register_associated_uri(response: &[u8]) -> Option<String> {
    RegisterArtifacts::parse(response)
        .default_associated_uri()
        .map(str::to_string)
}

fn mo_sms_uris(
    recipient: &str,
    service_center: &str,
    domain: &str,
) -> Result<(String, String), VolteError> {
    Ok((
        phone_uri(service_center, domain)?,
        phone_uri(recipient, domain)?,
    ))
}

fn phone_uri(number: &str, domain: &str) -> Result<String, VolteError> {
    let number = number.trim();
    if number.is_empty()
        || !number.chars().enumerate().all(|(index, character)| {
            character.is_ascii_digit() || (index == 0 && character == '+')
        })
    {
        return Err(VolteError::new("volte_phone_uri_invalid"));
    }
    Ok(format!("sip:{number}@{domain};user=phone"))
}

fn parse_digest_challenge(frame: &[u8]) -> Result<digest_aka::DigestChallenge, VolteError> {
    if let Some(value) = sip::header_value(frame, "WWW-Authenticate") {
        return digest_aka::parse_digest_challenge(&value, false);
    }
    if let Some(value) = sip::header_value(frame, "Proxy-Authenticate") {
        return digest_aka::parse_digest_challenge(&value, true);
    }
    Err(VolteError::new(code::DIGEST_CHALLENGE_MISSING))
}

async fn load_device_identity(
    device: &VolteDeviceBinding,
    profile_store: &ProfileStore,
    sim_override: &SimOverride,
) -> Result<DeviceIdentity, VolteError> {
    let modem = command_output(
        "mmcli",
        &["-m", device.modem_id.as_str(), "--output-keyvalue"],
    )
    .await?;
    let sim_path = key_value(&modem, "modem.generic.sim")
        .ok_or_else(|| VolteError::new(code::MM_IMSI_MISSING))?;
    let sim = command_output("mmcli", &["-i", &sim_path, "--output-keyvalue"]).await?;
    let sim_imsi = key_value(&sim, "sim.properties.imsi")
        .filter(|value| value.len() >= 5 && value.bytes().all(|byte| byte.is_ascii_digit()));
    let cimi_argument = "--command=AT+CIMI";
    let (imsi, imsi_source) = match command_output(
        "mmcli",
        &["-m", device.modem_id.as_str(), cimi_argument],
    )
    .await
    {
        Ok(output) => match identity::parse_cimi_response(&output) {
            Some(imsi) => (imsi, "at_cimi"),
            None => {
                tracing::warn!(
                    "Native VoLTE AT+CIMI response did not contain an IMSI; using SIM fallback"
                );
                (
                    sim_imsi.ok_or_else(|| VolteError::new(code::IMSI_MISSING))?,
                    "sim_imsi_fallback",
                )
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "Native VoLTE ModemManager AT+CIMI failed; using SIM IMSI fallback");
            (
                sim_imsi.ok_or_else(|| VolteError::new(code::IMSI_MISSING))?,
                "sim_imsi_fallback",
            )
        }
    };
    let home_plmn = [
        key_value(&sim, "sim.properties.operator-id"),
        key_value(&sim, "sim.properties.operator-identifier"),
        key_value(&modem, "modem.3gpp.operator-code"),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| valid_home_plmn(&imsi, candidate));
    let applications = match command_output(
        "qmicli",
        &[
            "-d",
            device.qmi_device.as_str(),
            "--device-open-proxy",
            "--uim-get-card-status",
        ],
    )
    .await
    {
        Ok(output) => identity::parse_uicc_applications(&output),
        Err(error) => {
            tracing::warn!(error = %error, "VoLTE UICC application discovery failed; using USIM AID fallback");
            identity::UiccApplications::default()
        }
    };
    let aka_aid = identity::resolve_usim_aid(applications.usim_aid.as_deref());
    let usim_aid = identity::aid_hex(&aka_aid);
    let isim_aid = applications.isim_aid.as_deref().map(identity::aid_hex);
    let resolved = profile_store
        .resolve_for_imsi_access(
            sim_override.ims_volte.profile_id.as_deref(),
            &imsi,
            home_plmn.as_deref(),
            CatalogAccessKind::LteEpc,
        )
        .map_err(|detail| VolteError::with_detail(code::CARRIER_PROFILE_MISSING, detail))?
        .ok_or_else(|| VolteError::new(code::CARRIER_PROFILE_MISSING))?;
    let profile = resolved.profile;
    let effective_ims = resolve_effective_ims_profile(profile, Some(sim_override));
    let effective_device_identity = resolve_effective_device_identity(
        Some(sim_override),
        (!device.equipment_identifier.trim().is_empty())
            .then_some(device.equipment_identifier.as_str()),
    );
    tracing::info!(
        profile_id = profile.meta.profile_id,
        profile_origin = resolved.origin.as_str(),
        ims_domain_source = ?effective_ims.domain.source,
        "Resolved native VoLTE carrier profile"
    );
    Ok(DeviceIdentity {
        ims: ImsIdentity {
            private_user: format!("{imsi}@{}", effective_ims.realm.value),
            public_uri: format!("sip:{imsi}@{}", effective_ims.domain.value),
            contact_user: imsi.clone(),
            home_domain: effective_ims.domain.value.clone(),
            contact_user_phone: false,
        },
        profile,
        effective_ims,
        effective_device_identity,
        aka_aid,
        usim_aid,
        source: match (imsi_source, isim_aid.is_some()) {
            ("at_cimi", true) => "at_cimi_isim_detected",
            ("at_cimi", false) => "at_cimi",
            (_, true) => "sim_imsi_fallback_isim_detected",
            (_, false) => "sim_imsi_fallback",
        },
        isim_aid,
    })
}

async fn resolve_device_binding(
    requested: &VolteDeviceBinding,
) -> Result<VolteDeviceBinding, VolteError> {
    for attempt in 0..MM_MODEM_WAIT_ATTEMPTS {
        if let Ok(details) = command_output(
            "mmcli",
            &["-m", requested.modem_id.as_str(), "--output-keyvalue"],
        )
        .await
        {
            if modem_is_ready(&details) {
                return Ok(requested.clone());
            }
        }

        if attempt + 1 < MM_MODEM_WAIT_ATTEMPTS {
            tokio::time::sleep(MM_MODEM_WAIT_DELAY).await;
        }
    }
    Err(VolteError::new(code::RUNTIME_MM_MODEM_WAIT_TIMEOUT))
}

fn modem_is_ready(output: &str) -> bool {
    matches!(
        key_value(output, "modem.generic.state").as_deref(),
        Some("registered" | "connected")
    )
}

async fn command_output(program: &str, args: &[&str]) -> Result<String, VolteError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("{program}:{error}"))
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " ");
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!(
                "{program}:{}:{}:{}",
                output.status.code().unwrap_or(-1),
                args.join(" "),
                stderr
            ),
        ))
    }
}

fn key_value(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key).then(|| value.trim().to_string())
    })
}

fn valid_home_plmn(imsi: &str, candidate: &str) -> bool {
    matches!(candidate.len(), 5 | 6)
        && candidate.bytes().all(|byte| byte.is_ascii_digit())
        && imsi.starts_with(candidate)
}

fn offered_security(send_port: u16, receive_port: u16) -> SecAgree {
    let spi = || {
        u32::from_str_radix(&sip::hex_token(4), 16)
            .ok()
            .filter(|value| *value != 0)
            // Keep the wire value inside RFC 3329 `1*8HEXDIG`. An 8-digit
            // decimal value parses identically as decimal or hex, so a strict
            // P-CSCF cannot reject it and a decimal-parsing peer cannot
            // misread the SPI (which would break the installed xfrm SA).
            .map(|value| value % 100_000_000)
            .filter(|value| *value != 0)
            .unwrap_or(1)
    };
    SecAgree {
        spi_c: spi(),
        spi_s: spi(),
        port_c: send_port,
        port_s: receive_port,
    }
}

fn new_sip_instance() -> String {
    let token = sip::hex_token(16);
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &token[0..8],
        &token[8..12],
        &token[12..16],
        &token[16..20],
        &token[20..32]
    )
}

fn sip_instance_for_profile(profile: &CarrierProfile, effective_imei: Option<&str>) -> String {
    if profile.identity.device_identity_enabled && profile.ims.register.always_add_sip_instance {
        if let Some(imei) = effective_imei
            .map(str::trim)
            .filter(|imei| crate::connectivity::core::device_identity::is_valid_imei(imei))
        {
            return format!("urn:imei:{imei}");
        }
    }
    new_sip_instance()
}

fn ensure_generation(runtime: &VolteRuntime, expected: u64) -> Result<(), VolteError> {
    if runtime.generation() == expected {
        Ok(())
    } else {
        Err(VolteError::new(code::RUNTIME_NOT_RUNNING))
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn to_ims_error(error: VolteError) -> ImsError {
    ImsError::new(error.code())
}

fn map_channel_error(error: ImsError) -> VolteError {
    VolteError::with_detail(code::IPSEC_UDP_BIND_FAILED, error.code())
}

fn map_register_error(error: ImsError) -> VolteError {
    let stage = match error.code() {
        "ims_register_initial_send_failed"
        | "ims_register_initial_receive_failed"
        | "ims_register_initial_unexpected_status" => code::REGISTER_INITIAL_UNEXPECTED_STATUS,
        _ => code::REGISTER_AUTH_UNEXPECTED_STATUS,
    };
    VolteError::with_detail(stage, error.code())
}

fn register_failure_status(failure: &RegisterFailure) -> Option<u16> {
    failure
        .response
        .as_deref()
        .and_then(|response| sip::parse_status(response).ok())
}

fn sec_agree_retry_variant(
    variant: VolteRegisterVariant,
    failure: &RegisterFailure,
) -> Option<VolteRegisterVariant> {
    if variant.policy.require_sec_agree
        || failure.auth_rounds != 0
        || register_failure_status(failure) != Some(421)
    {
        return None;
    }
    let response = failure.response.as_deref()?;
    response_requires_only_extension(response, "sec-agree").then(|| variant.requiring_sec_agree())
}

fn sec_agree_require_only_retry_variant(
    variant: VolteRegisterVariant,
    failure: &RegisterFailure,
) -> Option<VolteRegisterVariant> {
    (variant.server_required_sec_agree
        && variant.policy.require_sec_agree
        && variant.policy.proxy_require_sec_agree
        && variant.security_client_offer == VolteSecurityClientOffer::Compact
        && failure.auth_rounds == 0
        && register_failure_status(failure) == Some(400))
    .then(|| variant.requiring_sec_agree_without_proxy())
}

fn sec_agree_require_only_was_rejected(
    variant: VolteRegisterVariant,
    failure: &RegisterFailure,
) -> bool {
    variant.server_required_sec_agree
        && variant.policy.require_sec_agree
        && !variant.policy.proxy_require_sec_agree
        && failure.auth_rounds == 0
        && register_failure_status(failure) == Some(421)
        && failure
            .response
            .as_deref()
            .is_some_and(|response| response_requires_only_extension(response, "sec-agree"))
}

fn sec_agree_compact_security_retry_variant(
    variant: VolteRegisterVariant,
    failure: &RegisterFailure,
) -> Option<VolteRegisterVariant> {
    (variant.server_required_sec_agree
        && variant.policy.require_sec_agree
        && variant.policy.proxy_require_sec_agree
        && variant.security_client_offer == VolteSecurityClientOffer::FullSpaced
        && failure.auth_rounds == 0
        && register_failure_status(failure) == Some(400))
    .then(|| variant.with_compact_security_client())
}

fn sec_agree_spaced_security_retry_variant(
    variant: VolteRegisterVariant,
    failure: &RegisterFailure,
) -> Option<VolteRegisterVariant> {
    (variant.server_required_sec_agree
        && variant.policy.require_sec_agree
        && variant.policy.proxy_require_sec_agree
        && variant.security_client_offer == VolteSecurityClientOffer::Full
        && failure.auth_rounds == 0
        && register_failure_status(failure) == Some(400))
    .then(|| variant.with_spaced_security_client())
}

fn response_requires_only_extension(response: &[u8], supported_extension: &str) -> bool {
    let mut found = false;
    for value in sip::header_values(response, "Require") {
        for extension in value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            found = true;
            if !extension.eq_ignore_ascii_case(supported_extension) {
                return false;
            }
        }
    }
    found
}

fn terminal_register_failure_status(failure: &RegisterFailure) -> Option<u16> {
    matches!(
        failure.error.code(),
        "ims_register_initial_unexpected_status"
            | "ims_register_authenticated_unexpected_status"
            | "ims_register_auth_rejected"
    )
    .then(|| register_failure_status(failure))
    .flatten()
}

fn map_register_failure(failure: &RegisterFailure) -> VolteError {
    let mapped = map_register_error(failure.error);
    match terminal_register_failure_status(failure) {
        Some(status) => VolteError::with_detail(
            mapped.code(),
            format!("{}:sip_status={status}", failure.error.code()),
        ),
        None => mapped,
    }
}

fn should_retain_failed_bearer(error: &VolteError) -> bool {
    matches!(
        error.code(),
        code::REGISTER_INITIAL_UNEXPECTED_STATUS | code::REGISTER_AUTH_UNEXPECTED_STATUS
    ) && error
        .detail()
        .is_some_and(|detail| detail.contains("sip_status="))
}

fn log_volte_register_request_metadata(
    variant: VolteRegisterVariant,
    channel: &VolteSipChannel,
    request: &[u8],
) {
    let route = channel.route();
    tracing::info!(
        register_variant = variant.label,
        initial_authorization = variant.authorization.label(),
        request_uri = "home_registrar",
        local_family = ip_family_name(route.local_addr.ip()),
        local_port = route.local_addr.port(),
        pcscf = %route.pcscf_addr,
        route_header_present = !sip::header_values(request, "Route").is_empty(),
        authorization_present = !sip::header_values(request, "Authorization").is_empty(),
        security_client_present = !sip::header_values(request, "Security-Client").is_empty(),
        negotiated_security_client_offer = ?variant.security_client_offer,
        p_preferred_identity_present =
            !sip::header_values(request, "P-Preferred-Identity").is_empty(),
        pani_present = !sip::header_values(request, "P-Access-Network-Info").is_empty(),
        visited_network_present = !sip::header_values(request, "P-Visited-Network-ID").is_empty(),
        sec_agree_advertised = variant.policy.advertise_sec_agree,
        sec_agree_required = variant.policy.require_sec_agree,
        proxy_sec_agree_required = variant.policy.proxy_require_sec_agree,
        mmtel_features_present = variant.policy.include_mmtel_features,
        sms_over_ip_advertised = sip::header_values(request, "Contact")
            .iter()
            .any(|contact| contact.to_ascii_lowercase().contains("+g.3gpp.smsip")),
        accept_contact_count = sip::header_values(request, "Accept-Contact").len(),
        request_bytes = request.len(),
        sensitive_values = "redacted",
        "VoLTE IMS REGISTER request metadata prepared"
    );
}

fn log_volte_register_failure_metadata(variant: VolteRegisterVariant, failure: &RegisterFailure) {
    let Some(response) = failure.response.as_deref() else {
        tracing::warn!(
            register_variant = variant.label,
            error = failure.error.code(),
            auth_rounds = failure.auth_rounds,
            "VoLTE IMS REGISTER failed before a complete response"
        );
        return;
    };
    tracing::warn!(
        register_variant = variant.label,
        error = failure.error.code(),
        sip_status = ?register_failure_status(failure),
        auth_rounds = failure.auth_rounds,
        digest_challenge_present = !sip::header_values(response, "WWW-Authenticate").is_empty()
            || !sip::header_values(response, "Proxy-Authenticate").is_empty(),
        security_server_count = sip::header_values(response, "Security-Server").len(),
        warning_present = !sip::header_values(response, "Warning").is_empty(),
        unsupported_present = !sip::header_values(response, "Unsupported").is_empty(),
        require_present = !sip::header_values(response, "Require").is_empty(),
        proxy_require_present = !sip::header_values(response, "Proxy-Require").is_empty(),
        sensitive_values = "redacted",
        "VoLTE IMS REGISTER terminal response metadata received"
    );
}

fn ip_family_name(address: IpAddr) -> &'static str {
    if address.is_ipv6() {
        "ipv6"
    } else {
        "ipv4"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::voice::MediaDirection;

    fn register_variant(label: &str) -> VolteRegisterVariant {
        *VOLTE_REGISTER_VARIANTS
            .iter()
            .find(|variant| variant.label == label)
            .unwrap_or_else(|| panic!("missing REGISTER variant: {label}"))
    }

    #[test]
    fn modemmanager_prefers_explicit_profile_before_apn_only_fallback() {
        let request = BearerRequest {
            apn: "ims".into(),
            allow_roaming: true,
            profile_id: Some(2),
        };
        let candidates = modemmanager_bearer_requests(&request);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].profile_id, Some(2));
        assert_eq!(candidates[1].profile_id, None);

        let unbound = BearerRequest::for_apn("ims", true);
        assert_eq!(modemmanager_bearer_requests(&unbound), vec![unbound]);
    }

    #[test]
    fn modemmanager_profile_fallback_never_retries_a_wedged_baseband() {
        let rejected = VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "verbose call end reason (6,32): option-not-supported",
        );
        assert!(may_retry_modemmanager_profile_binding(&rejected));

        let wedged =
            VolteError::with_detail(code::RUNTIME_MM_BEARER_CONNECT_FAILED, "endpoint hangup");
        assert!(!may_retry_modemmanager_profile_binding(&wedged));
    }

    fn test_audio_offer(endpoint: SocketAddr, direction: MediaDirection) -> MediaOffer {
        let sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na={}\r\n",
            endpoint.port(),
            direction.as_str()
        );
        MediaOffer {
            audio_endpoint: endpoint,
            audio: parse_audio_sdp(sdp.as_bytes()).unwrap(),
            video: None,
            dtmf: DtmfCapabilities {
                rtp_event: Some(RtpTelephoneEvent {
                    payload_type: 101,
                    clock_rate: 8000,
                    events: Some("0-16".into()),
                }),
                sip_info: true,
                preferred: DtmfSource::RtpEvent,
            },
        }
    }

    fn test_network_audio_sdp(endpoint: SocketAddr, direction: MediaDirection) -> String {
        format!(
            "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=fmtp:96 0-16\r\na={}\r\n",
            endpoint.port(),
            direction.as_str()
        )
    }

    async fn recv_test_sip(socket: &tokio::net::UdpSocket) -> Vec<u8> {
        let mut frame = vec![0u8; 65_535];
        let (len, _) = tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut frame))
            .await
            .expect("timed out waiting for VoLTE SIP frame")
            .unwrap();
        frame.truncate(len);
        frame
    }

    async fn test_voice_session() -> (VolteLiveHandle, Arc<VolteRuntime>, tokio::net::UdpSocket) {
        let pcscf = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let pcscf_addr = pcscf.local_addr().unwrap();
        let channel = VolteSipChannel::bind(
            ImsRoute {
                local_addr: "127.0.0.1:0".parse().unwrap(),
                pcscf_addr,
                transport: SipTransport::Udp,
            },
            None,
            None,
        )
        .unwrap();
        let profile = &crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        let live = VolteLiveHandle::new();
        *live.session.lock().await = Some(VolteLiveSession {
            channel,
            identity: ImsIdentity {
                private_user: "234330000000001@ims.example".into(),
                public_uri: "sip:+441234567890@ims.example".into(),
                contact_user: "234330000000001".into(),
                home_domain: "ims.example".into(),
                contact_user_phone: false,
            },
            registration: RegisteredImsContext::from_response(
                ImsRegistrationAccess::Volte,
                b"SIP/2.0 200 OK\r\nExpires: 3600\r\nContent-Length: 0\r\n\r\n",
                3600,
            ),
            bearer: BearerConnection {
                path: "/org/freedesktop/ModemManager1/Bearer/1".into(),
                interface: "lo".into(),
                ip_type: "ipv4".into(),
                settings: crate::connectivity::modems::ims::volte::pcscf::ImsIpSettings {
                    ipv4_address: Some("127.0.0.1".parse().unwrap()),
                    ..Default::default()
                },
                ipv4_prefix: Some(8),
                ipv6_prefix: None,
                mtu: None,
            },
            native_bearer: None,
            data_slot_mode: DataSlotMode::PrimaryImsOnly,
            pcscf_reporting_cid: None,
            ims_profile_lease: None,
            pcscf: pcscf_addr,
            ip_family: "ipv4",
            xfrm_plan: None,
            register_ids: RequestIds::fresh(1),
            next_register_cseq: 2,
            sip_instance: "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
            security_binding: SecAgree {
                spi_c: 1,
                spi_s: 2,
                port_c: 5060,
                port_s: 5062,
            },
            register_variant: register_variant("reference_sms_sec_agree"),
            device: VolteDeviceBinding {
                line_id: "volte-dialog-matrix".into(),
                modem_id: "test".into(),
                qmi_device: "/dev/null".into(),
                uim_slot: 1,
                equipment_identifier: "490154203237518".into(),
            },
            aka_aid: Vec::new(),
            profile,
            effective_ims: resolve_effective_ims_profile(profile, None),
            voice_calls: HashMap::new(),
            mwi_subscription: None,
        });
        (live, Arc::new(VolteRuntime::new()), pcscf)
    }

    #[test]
    fn reference_sms_sec_agree_variant_is_attempted_first() {
        let first = VOLTE_REGISTER_VARIANTS[0];

        assert_eq!(first.label, "reference_sms_sec_agree");
        assert_eq!(first.authorization, VolteInitialAuthorization::None);
        assert_eq!(first.policy, sip::RegisterRequestPolicy::LEGACY);
        assert_eq!(first.security_client_offer, VolteSecurityClientOffer::Full);
    }

    #[test]
    fn bearer_backend_follows_the_selected_ims_endpoint() {
        assert!(!native_ims_bearer_required(DataSlotMode::PrimaryImsOnly));
        assert!(!native_ims_bearer_required(
            DataSlotMode::PrimaryImsSecondaryData
        ));
        assert!(native_ims_bearer_required(
            DataSlotMode::SecondaryImsPrimaryData
        ));
    }

    #[test]
    fn primary_ims_with_secondary_data_does_not_pre_activate_the_profile() {
        assert!(!active_ims_profile_prefetch_required(
            DataSlotMode::PrimaryImsSecondaryData
        ));
    }

    #[test]
    fn secondary_ims_with_primary_data_keeps_native_profile_prefetch() {
        assert!(active_ims_profile_prefetch_required(
            DataSlotMode::SecondaryImsPrimaryData
        ));
    }

    #[test]
    fn device_binding_uses_discovered_modem_qmi_and_slot() {
        let modem = ModemBinding {
            line_id: "line-0123456789abcdef0123456789abcdef".to_string(),
            modem_id: "7".to_string(),
            qmi_device: Some("/dev/cdc-wdm3".to_string()),
            uim_slot: 2,
            equipment_identifier: "490154203237518".to_string(),
            ..ModemBinding::default()
        };
        let device = VolteDeviceBinding::from_modem(&modem).unwrap();
        assert_eq!(device.modem_id, "7");
        assert_eq!(device.qmi_device, "/dev/cdc-wdm3");
        assert_eq!(device.uim_slot, 2);
        assert_eq!(device.equipment_identifier, "490154203237518");
    }

    #[test]
    fn sip_instance_imei_is_strictly_carrier_policy_gated() {
        let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
        profile.ims.register.always_add_sip_instance = true;
        profile.identity.device_identity_enabled = false;
        assert!(
            sip_instance_for_profile(&profile, Some("490154203237518")).starts_with("urn:uuid:")
        );

        profile.identity.device_identity_enabled = true;
        assert_eq!(
            sip_instance_for_profile(&profile, Some("490154203237518")),
            "urn:imei:490154203237518"
        );
        assert!(sip_instance_for_profile(&profile, Some("12345")).starts_with("urn:uuid:"));
        assert!(sip_instance_for_profile(&profile, None).starts_with("urn:uuid:"));
    }

    #[test]
    fn device_binding_rejects_modem_without_qmi_control_port() {
        assert!(VolteDeviceBinding::from_modem(&ModemBinding::default()).is_err());
    }

    #[test]
    fn parses_device_identity_without_serializing_it() {
        let modem = "modem.generic.sim : /org/freedesktop/ModemManager1/SIM/0\nmodem.3gpp.operator-code : 46011\n";
        assert_eq!(
            key_value(modem, "modem.3gpp.operator-code").as_deref(),
            Some("46011")
        );
        assert!(!format!("{modem:?}").contains("460111234567890"));
    }

    #[test]
    fn modem_readiness_waits_for_registration() {
        assert!(modem_is_ready("modem.generic.state : registered\n"));
        assert!(modem_is_ready("modem.generic.state : connected\n"));
        assert!(!modem_is_ready("modem.generic.state : enabling\n"));
        assert!(!modem_is_ready("modem.generic.state : disabled\n"));
    }

    #[test]
    fn prefix_unavailable_failure_is_classified_distinctly() {
        // The bearer prefix-unavailable failure is classified on its own so the
        // family-fallback logic can react to it. (The old AT-context cleanup-and-
        // retry workaround that keyed off this was removed with the beta2 P-CSCF
        // reordering, since AT no longer runs before the bearer.)
        let prefix = VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "volte_command_failed:mmcli:prefix-unavailable",
        );
        assert_eq!(
            FailureClass::from_details(prefix.detail().unwrap_or("")),
            FailureClass::PrefixUnavailable
        );
        let generic = VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "volte_command_failed:mmcli:operation-failed",
        );
        assert_ne!(
            FailureClass::from_details(generic.detail().unwrap_or("")),
            FailureClass::PrefixUnavailable
        );
        assert_ne!(
            FailureClass::from_details(
                VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED)
                    .detail()
                    .unwrap_or("")
            ),
            FailureClass::PrefixUnavailable
        );
    }

    #[test]
    fn security_offer_contains_nonzero_spis_and_distinct_send_receive_ports() {
        let offer = offered_security(42799, 45652);
        let parsed = ipsec::parse_security_server(&offer.security_client_value()).unwrap();
        assert_ne!(parsed.spi_c, 0);
        assert_ne!(parsed.spi_s, 0);
        assert_eq!(parsed.port_c, 42799);
        assert_eq!(parsed.port_s, 45652);
    }

    #[test]
    fn phone_uri_accepts_e164_and_rejects_non_phone_text() {
        assert_eq!(
            phone_uri("+8613800138000", "ims.example").unwrap(),
            "sip:+8613800138000@ims.example;user=phone"
        );
        assert!(phone_uri("+86-138", "ims.example").is_err());
        assert!(phone_uri("", "ims.example").is_err());
    }

    #[test]
    fn mo_sms_routes_via_service_center_but_targets_recipient() {
        let (request_uri, to_uri) =
            mo_sms_uris("+8619399144749", "+8613800100500", "ims.example").unwrap();
        assert_eq!(request_uri, "sip:+8613800100500@ims.example;user=phone");
        assert_eq!(to_uri, "sip:+8619399144749@ims.example;user=phone");
    }

    #[test]
    fn register_service_route_is_preserved_for_later_requests() {
        let response = b"SIP/2.0 200 OK\r\nService-Route: <sip:pcscf.example:9900;lr>\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            register_service_route(response).as_deref(),
            Some("<sip:pcscf.example:9900;lr>")
        );
        assert!(register_service_route(b"SIP/2.0 200 OK\r\n\r\n").is_none());
    }

    #[test]
    fn register_default_associated_uri_is_preserved_for_later_requests() {
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: <sip:+8613800138000@ims.example;user=phone>, <tel:+8613800138000>\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            register_associated_uri(response).as_deref(),
            Some("sip:+8613800138000@ims.example;user=phone")
        );
        assert!(register_associated_uri(b"SIP/2.0 200 OK\r\n\r\n").is_none());
    }

    #[test]
    fn register_associated_uri_accepts_repeated_bare_tel_header() {
        let response = b"SIP/2.0 200 OK\r\nP-Associated-URI: tel:+8613800138000\r\nP-Associated-URI: <sip:460001234567890@ims.example>\r\n\r\n";
        assert_eq!(
            register_associated_uri(response).as_deref(),
            Some("tel:+8613800138000")
        );
    }

    #[test]
    fn digest_challenge_prefers_www_then_proxy() {
        let frame = b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"YWJj\",algorithm=AKAv1-MD5\r\nContent-Length: 0\r\n\r\n";
        let challenge = parse_digest_challenge(frame).unwrap();
        assert_eq!(challenge.realm, "ims.example");
        assert!(!challenge.proxy);
    }

    #[test]
    fn sec_agree_421_upgrades_the_same_ims_register_variant() {
        let failure = RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"),
            response: Some(
                b"SIP/2.0 421 Extension Required\r\nRequire: SEC-AGREE\r\nContent-Length: 0\r\n\r\n"
                    .to_vec(),
            ),
            auth_rounds: 0,
        };
        let base = register_variant("ims_features_aka_uri_first");
        let upgraded = sec_agree_retry_variant(base, &failure).unwrap();

        assert_eq!(
            upgraded.label,
            "ims_features_aka_uri_first_sec_agree_required"
        );
        assert_eq!(upgraded.authorization, base.authorization);
        assert_eq!(
            upgraded.policy,
            sip::RegisterRequestPolicy {
                advertise_sec_agree: true,
                require_sec_agree: true,
                proxy_require_sec_agree: true,
                ..base.policy
            }
        );

        let identity = ImsIdentity {
            private_user: "460001234567890@ims.example".to_string(),
            public_uri: "sip:460001234567890@ims.example".to_string(),
            contact_user: "460001234567890".to_string(),
            home_domain: "ims.example".to_string(),
            contact_user_phone: false,
        };
        let route = ImsRoute {
            local_addr: "192.0.2.2:5060".parse().unwrap(),
            pcscf_addr: "192.0.2.1:5060".parse().unwrap(),
            transport: SipTransport::Udp,
        };
        let authorization = upgraded
            .authorization
            .build(
                crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433
                    .ims
                    .realm,
                &identity,
                "sip:ims.example",
            )
            .unwrap();
        let request = sip::build_register_with_policy(
            &identity,
            &route,
            &RequestIds::fresh(1),
            3600,
            Some(&authorization),
            Some("ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=2;port-c=5060;port-s=5062"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
            upgraded.policy,
        );

        assert!(sip::header_value(&request, "Authorization")
            .is_some_and(|value| value.starts_with("Digest uri=\"sip:ims.example\",username=")));
        assert!(sip::header_value(&request, "Contact")
            .is_some_and(|value| value.contains(";+g.3gpp.icsi-ref=")));
        assert!(sip::header_values(&request, "Accept-Contact").is_empty());
        assert_eq!(
            sip::header_value(&request, "Route").as_deref(),
            Some("<sip:192.0.2.1:5060;lr>")
        );
        assert_eq!(
            sip::header_value(&request, "P-Visited-Network-ID").as_deref(),
            Some("\"ims.example\"")
        );
        assert_eq!(
            sip::header_value(&request, "Require").as_deref(),
            Some("sec-agree")
        );
        assert_eq!(
            sip::header_value(&request, "Proxy-Require").as_deref(),
            Some("sec-agree")
        );
    }

    #[test]
    fn server_required_sec_agree_400_retries_formats_then_without_proxy_require() {
        let failure = RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"),
            response: Some(b"SIP/2.0 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec()),
            auth_rounds: 0,
        };
        let base = register_variant("ims_features_aka_uri_first");
        assert!(sec_agree_require_only_retry_variant(base, &failure).is_none());
        assert!(sec_agree_spaced_security_retry_variant(base, &failure).is_none());
        assert!(sec_agree_compact_security_retry_variant(base, &failure).is_none());

        let require_and_proxy = base.requiring_sec_agree();
        assert!(sec_agree_require_only_retry_variant(require_and_proxy, &failure).is_none());
        assert!(sec_agree_compact_security_retry_variant(require_and_proxy, &failure).is_none());
        let spaced_security =
            sec_agree_spaced_security_retry_variant(require_and_proxy, &failure).unwrap();
        assert_eq!(
            spaced_security.label,
            "ims_features_aka_uri_first_sec_agree_spaced_security"
        );
        assert_eq!(
            spaced_security.security_client_offer,
            VolteSecurityClientOffer::FullSpaced
        );
        let compact_security =
            sec_agree_compact_security_retry_variant(spaced_security, &failure).unwrap();
        assert_eq!(
            compact_security.label,
            "ims_features_aka_uri_first_sec_agree_compact_security"
        );
        assert_eq!(
            compact_security.security_client_offer,
            VolteSecurityClientOffer::Compact
        );
        let require_only =
            sec_agree_require_only_retry_variant(compact_security, &failure).unwrap();
        assert_eq!(
            require_only.label,
            "ims_features_aka_uri_first_sec_agree_require_only"
        );
        assert_eq!(require_only.authorization, base.authorization);
        assert!(require_only.policy.require_sec_agree);
        assert!(!require_only.policy.proxy_require_sec_agree);
        assert!(require_only.policy.include_mmtel_features);
        assert!(require_only.policy.include_route_header);

        let require_only_rejection = RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"),
            response: Some(
                b"SIP/2.0 421 Extension Required\r\nRequire: sec-agree\r\nContent-Length: 0\r\n\r\n"
                    .to_vec(),
            ),
            auth_rounds: 0,
        };
        assert!(sec_agree_require_only_was_rejected(
            require_only,
            &require_only_rejection
        ));
        assert!(!sec_agree_require_only_was_rejected(
            base,
            &require_only_rejection
        ));

        let identity = ImsIdentity {
            private_user: "460001234567890@ims.example".to_string(),
            public_uri: "sip:460001234567890@ims.example".to_string(),
            contact_user: "460001234567890".to_string(),
            home_domain: "ims.example".to_string(),
            contact_user_phone: false,
        };
        let request = sip::build_register_with_policy(
            &identity,
            &ImsRoute {
                local_addr: "192.0.2.2:5060".parse().unwrap(),
                pcscf_addr: "192.0.2.1:5060".parse().unwrap(),
                transport: SipTransport::Udp,
            },
            &RequestIds::fresh(1),
            3600,
            require_only
                .authorization
                .build(
                    crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433
                        .ims
                        .realm,
                    &identity,
                    "sip:ims.example",
                )
                .as_deref(),
            Some("ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=2;port-c=5060;port-s=5062"),
            None,
            "urn:uuid:00000000-0000-4000-8000-000000000000",
            require_only.policy,
        );
        assert_eq!(
            sip::header_value(&request, "Require").as_deref(),
            Some("sec-agree")
        );
        assert!(sip::header_value(&request, "Proxy-Require").is_none());
        assert!(sip::header_value(&request, "Authorization")
            .is_some_and(|value| value.starts_with("Digest uri=\"sip:ims.example\",username=")));
        assert_eq!(
            sip::header_value(&request, "Route").as_deref(),
            Some("<sip:192.0.2.1:5060;lr>")
        );
    }

    #[test]
    fn other_421_requirements_do_not_select_a_retry_variant() {
        for require in ["timer", "sec-agree, timer"] {
            let failure = RegisterFailure {
                error: ImsError::new("ims_register_initial_unexpected_status"),
                response: Some(
                    format!(
                        "SIP/2.0 421 Extension Required\r\nRequire: {require}\r\nContent-Length: 0\r\n\r\n"
                    )
                    .into_bytes(),
                ),
                auth_rounds: 0,
            };

            assert!(sec_agree_retry_variant(
                register_variant("ims_features_aka_uri_first"),
                &failure
            )
            .is_none());
        }
    }

    #[test]
    fn register_transport_errors_preserve_initial_vs_authenticated_stage() {
        let initial = map_register_error(ImsError::new("ims_register_initial_receive_failed"));
        assert_eq!(initial.code(), code::REGISTER_INITIAL_UNEXPECTED_STATUS);
        assert_eq!(
            initial.detail(),
            Some("ims_register_initial_receive_failed")
        );

        let authenticated =
            map_register_error(ImsError::new("ims_register_authenticated_receive_failed"));
        assert_eq!(authenticated.code(), code::REGISTER_AUTH_UNEXPECTED_STATUS);
        assert_eq!(
            authenticated.detail(),
            Some("ims_register_authenticated_receive_failed")
        );
    }

    #[test]
    fn bearer_retention_requires_a_terminal_register_response() {
        let terminal = RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"),
            response: Some(b"SIP/2.0 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec()),
            auth_rounds: 0,
        };
        let terminal_error = map_register_failure(&terminal);
        assert!(should_retain_failed_bearer(&terminal_error));
        assert!(terminal_error
            .detail()
            .is_some_and(|detail| detail.contains("sip_status=400")));

        let local_auth_failure = RegisterFailure {
            error: ImsError::new(code::USIM_AKA_FAILED),
            response: Some(b"SIP/2.0 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec()),
            auth_rounds: 1,
        };
        let local_error = map_register_failure(&local_auth_failure);
        assert!(!should_retain_failed_bearer(&local_error));
        assert!(!local_error
            .detail()
            .is_some_and(|detail| detail.contains("sip_status=")));
    }

    #[test]
    fn family_fallback_is_limited_to_discovery_and_initial_transport_failures() {
        assert!(
            FailureClass::from_error(&VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED))
                .is_retryable_family()
        );
        assert!(FailureClass::from_error(&VolteError::new(
            code::REGISTER_INITIAL_UNEXPECTED_STATUS
        ))
        .is_retryable_family());
        assert!(
            !FailureClass::from_error(&VolteError::new(code::REGISTER_AUTH_UNEXPECTED_STATUS))
                .is_retryable_family()
        );
        assert!(
            !FailureClass::from_error(&VolteError::new(code::USIM_AKA_FAILED))
                .is_retryable_family()
        );
        assert_eq!(ip_family_name("2001:db8::1".parse().unwrap()), "ipv6");
        assert_eq!(ip_family_name("192.0.2.1".parse().unwrap()), "ipv4");
    }

    #[test]
    fn operator_callee_uses_request_user_and_ims_home_domain() {
        assert_eq!(
            normalize_operator_callee("sip:+8613800138000@10.0.0.116", "ims.example").unwrap(),
            "sip:+8613800138000@ims.example;user=phone"
        );
        assert_eq!(
            normalize_operator_callee("*86", "ims.example").unwrap(),
            "sip:*86@ims.example;user=phone"
        );
        assert!(normalize_operator_callee("sip:not-a-number@pbx", "ims.example").is_err());
        assert_eq!(
            normalize_incoming_caller("tel:+8613800138000"),
            "sip:+8613800138000@simadmin"
        );
        assert_eq!(
            normalize_incoming_caller("sips:+8613800138000@ims.example"),
            "sip:+8613800138000@ims.example"
        );
    }

    #[test]
    fn relay_sdp_advertises_allocated_endpoint_and_dtmf() {
        let source = b"v=0\r\no=- 1 1 IN IP4 10.0.0.3\r\ns=call\r\nc=IN IP4 10.0.0.3\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
        let audio = parse_audio_sdp(source).unwrap();
        let sdp = relay_audio_sdp(
            &audio,
            Some(&RtpTelephoneEvent {
                payload_type: 101,
                clock_rate: 8000,
                events: Some("0-16".into()),
            }),
            "192.0.2.10:32000".parse().unwrap(),
        );
        assert!(sdp.contains("c=IN IP4 192.0.2.10\r\n"));
        assert!(sdp.contains("m=audio 32000 RTP/AVP 0 101\r\n"));
        assert!(sdp.contains("a=rtpmap:101 telephone-event/8000\r\n"));
        assert!(sdp.contains("a=fmtp:101 0-16\r\n"));
    }

    /// A reliable provisional response carries the SDP answer, and the operator's
    /// final 200 OK then arrives with an empty body. The retained early answer
    /// must be reused so the call is not torn down as unusable media.
    #[tokio::test]
    async fn empty_final_answer_reuses_early_provisional_answer() {
        let operator_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_audio = parse_audio_sdp(
            format!(
                "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n",
                internal_remote.local_addr().unwrap().port()
            )
            .as_bytes(),
        )
        .unwrap();
        let internal_offer = MediaOffer {
            audio: internal_audio,
            audio_endpoint: internal_remote.local_addr().unwrap(),
            video: None,
            dtmf: DtmfCapabilities {
                rtp_event: None,
                sip_info: true,
                preferred: DtmfSource::SipInfo,
            },
        };
        let pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let operator_local = pending.operator_local_addr().unwrap();
        let internal_local = pending.internal_local_addr().unwrap();
        let mut call = LiveVoiceCall {
            direction: LiveVoiceDirection::MobileOriginated,
            dialog: sip::DialogIds::fresh(),
            callee_uri: "sip:+601112023012@ims.example;user=phone".into(),
            invite_branch: "z9hG4bKearly".into(),
            initial_invite: None,
            internal_offer,
            operator_local,
            internal_local,
            pending_relay: Some(pending),
            active_relay: None,
            ip_answer_wait_armed: false,
            operator_answered: false,
            next_cseq: 2,
            media_metrics: None,
            pending_operator_reinvite: None,
            pending_asterisk_reinvite: false,
            pending_video_relay: None,
            active_video_relay: None,
            operator_video_local: None,
            internal_video_local: None,
            pending_media_rollback: None,
            renegotiation_deadline: None,
            early_answer: None,
            transfer: None,
            transfer_deadline: None,
        };

        // The operator answers inside the reliable 183, exactly as Maxis does.
        let provisional_sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n",
            operator_remote.local_addr().unwrap().port()
        );
        let early = prepare_operator_media(&mut call, provisional_sdp.as_bytes()).unwrap();
        call.early_answer = Some(early.clone());
        assert!(early.contains("m=audio"));

        // The final 200 OK has no SDP, so the production final-answer path reuses
        // the answer negotiated from the reliable provisional response.
        assert_eq!(prepare_final_operator_media(&mut call, b"").unwrap(), early);
    }

    #[test]
    fn relay_sdp_preserves_independent_video_addressing() {
        let source = b"v=0\r\no=- 1 1 IN IP4 10.0.0.3\r\ns=call\r\nc=IN IP4 10.0.0.3\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\nm=video 50000 RTP/AVP 99\r\nc=IN IP4 10.0.0.4\r\na=rtpmap:99 H264/90000\r\na=sendrecv\r\n";
        let audio = parse_audio_sdp(source).unwrap();
        let video = parse_video_sdp(source).unwrap();
        assert_eq!(
            media_endpoint_for_video(&audio, &video).unwrap(),
            "10.0.0.4:50000".parse().unwrap()
        );
        let offer = MediaOffer {
            audio_endpoint: "10.0.0.3:40000".parse().unwrap(),
            audio,
            video: Some(VideoOffer {
                description: video,
                endpoint: "10.0.0.4:50000".parse().unwrap(),
            }),
            dtmf: DtmfCapabilities {
                rtp_event: None,
                sip_info: true,
                preferred: DtmfSource::SipInfo,
            },
        };
        let sdp = relay_media_sdp(
            &offer,
            "192.0.2.10:32000".parse().unwrap(),
            Some("198.51.100.20:33000".parse().unwrap()),
        );
        assert!(sdp.contains("c=IN IP4 192.0.2.10\r\n"));
        assert!(sdp.contains("m=video 33000 RTP/AVP 99\r\nc=IN IP4 198.51.100.20\r\n"));
    }

    #[tokio::test]
    async fn two_dialogs_keep_progress_media_dtmf_and_reinvite_state_independent() {
        let (live, runtime, pcscf) = test_voice_session().await;
        let mut events = live.operator.subscribe_events();
        let trunk_rtp_a = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let trunk_rtp_b = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let trunk_rtp_c = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let network_rtp_a = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let network_rtp_c = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();

        for (call_id, callee, endpoint) in [
            (
                "matrix-call-a",
                "+441234567891",
                trunk_rtp_a.local_addr().unwrap(),
            ),
            (
                "matrix-call-b",
                "+441234567892",
                trunk_rtp_b.local_addr().unwrap(),
            ),
        ] {
            handle_operator_command(
                &live,
                &runtime,
                OperatorCommand::StartCall {
                    call_id: call_id.into(),
                    caller: "6108".into(),
                    callee: callee.into(),
                    trunk_local_ip: "127.0.0.1".parse().unwrap(),
                    offer: test_audio_offer(endpoint, MediaDirection::SendRecv),
                },
            )
            .await
            .unwrap();
        }
        let invite_a = recv_test_sip(&pcscf).await;
        let invite_b = recv_test_sip(&pcscf).await;
        let ims_call_a = sip::header_value(&invite_a, "Call-ID").unwrap();
        let ims_call_b = sip::header_value(&invite_b, "Call-ID").unwrap();
        let relay_a =
            media_socket_addr(&parse_audio_sdp(sip::sip_body(&invite_a)).unwrap()).unwrap();
        assert_ne!(ims_call_a, ims_call_b);

        let ringing_a =
            sip::build_response(&invite_a, 180, "Ringing", Some("network-a"), None, None);
        assert!(handle_operator_sip_frame(&live, &runtime, &ringing_a)
            .await
            .unwrap());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Provisional { call_id, status: 180, body: None }
                if call_id == "matrix-call-a"
        ));

        let busy_b =
            sip::build_response(&invite_b, 486, "Busy Here", Some("network-b"), None, None);
        assert!(handle_operator_sip_frame(&live, &runtime, &busy_b)
            .await
            .unwrap());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Rejected { call_id, status: 486 }
                if call_id == "matrix-call-b"
        ));

        // Reuse the rejected slot while call A remains in its original dialog.
        handle_operator_command(
            &live,
            &runtime,
            OperatorCommand::StartCall {
                call_id: "matrix-call-c".into(),
                caller: "6108".into(),
                callee: "+441234567893".into(),
                trunk_local_ip: "127.0.0.1".parse().unwrap(),
                offer: test_audio_offer(
                    trunk_rtp_c.local_addr().unwrap(),
                    MediaDirection::SendRecv,
                ),
            },
        )
        .await
        .unwrap();
        let invite_c = recv_test_sip(&pcscf).await;
        let ims_call_c = sip::header_value(&invite_c, "Call-ID").unwrap();
        let relay_c =
            media_socket_addr(&parse_audio_sdp(sip::sip_body(&invite_c)).unwrap()).unwrap();
        assert_ne!(ims_call_a, ims_call_c);
        assert_ne!(relay_a, relay_c);

        let answer_a = test_network_audio_sdp(
            network_rtp_a.local_addr().unwrap(),
            MediaDirection::SendRecv,
        );
        let answer_c = test_network_audio_sdp(
            network_rtp_c.local_addr().unwrap(),
            MediaDirection::SendRecv,
        );
        let progress_c = sip::build_response(
            &invite_c,
            183,
            "Session Progress",
            Some("network-c"),
            None,
            Some(answer_c.as_bytes()),
        );
        assert!(handle_operator_sip_frame(&live, &runtime, &progress_c)
            .await
            .unwrap());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Provisional { call_id, status: 183, body: Some(_) }
                if call_id == "matrix-call-c"
        ));

        for (invite, answer, call_id, ims_call_id, tag) in [
            (
                &invite_a,
                answer_a.as_bytes(),
                "matrix-call-a",
                ims_call_a.as_str(),
                "network-a",
            ),
            (
                &invite_c,
                answer_c.as_bytes(),
                "matrix-call-c",
                ims_call_c.as_str(),
                "network-c",
            ),
        ] {
            let accepted = sip::build_response(invite, 200, "OK", Some(tag), None, Some(answer));
            assert!(handle_operator_sip_frame(&live, &runtime, &accepted)
                .await
                .unwrap());
            let ack = recv_test_sip(&pcscf).await;
            assert!(ack.starts_with(b"ACK "));
            assert_eq!(
                sip::header_value(&ack, "Call-ID").as_deref(),
                Some(ims_call_id)
            );
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(1), events.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                OperatorEvent::Answered { call_id: answered, .. } if answered == call_id
            ));
        }

        let hold_rtp = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        handle_operator_command(
            &live,
            &runtime,
            OperatorCommand::Renegotiate {
                call_id: "matrix-call-a".into(),
                trunk_local_ip: "127.0.0.1".parse().unwrap(),
                offer: test_audio_offer(hold_rtp.local_addr().unwrap(), MediaDirection::Inactive),
            },
        )
        .await
        .unwrap();
        let hold = recv_test_sip(&pcscf).await;
        assert_eq!(
            sip::header_value(&hold, "Call-ID").as_deref(),
            Some(ims_call_a.as_str())
        );
        assert_eq!(
            parse_audio_sdp(sip::sip_body(&hold)).unwrap().direction,
            MediaDirection::Inactive
        );
        let inactive = test_network_audio_sdp(
            network_rtp_a.local_addr().unwrap(),
            MediaDirection::Inactive,
        );
        let held = sip::build_response(
            &hold,
            200,
            "OK",
            Some("network-a"),
            None,
            Some(inactive.as_bytes()),
        );
        assert!(handle_operator_sip_frame(&live, &runtime, &held)
            .await
            .unwrap());
        assert!(recv_test_sip(&pcscf).await.starts_with(b"ACK "));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, body }
                if call_id == "matrix-call-a"
                    && parse_audio_sdp(&body).unwrap().direction == MediaDirection::Inactive
        ));

        // Call C still carries RTP and DTMF while A is held.
        let packet = crate::connectivity::core::voice::RtpPacket {
            payload_type: 0,
            marker: false,
            sequence: 7,
            timestamp: 1120,
            ssrc: 0x0102_0304,
            payload: vec![0xaa, 0xbb],
        }
        .encode();
        network_rtp_c.send_to(&packet, relay_c).await.unwrap();
        let mut received = [0u8; 256];
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(1), trunk_rtp_c.recv_from(&mut received))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&received[..len], packet.as_slice());

        handle_operator_command(
            &live,
            &runtime,
            OperatorCommand::SendDtmf {
                call_id: "matrix-call-c".into(),
                signal: crate::services::trunk::bridge::DtmfSignal {
                    digit: '7',
                    duration_ms: 200,
                    source: DtmfSource::SipInfo,
                },
            },
        )
        .await
        .unwrap();
        let info_c = recv_test_sip(&pcscf).await;
        assert!(info_c.starts_with(b"INFO "));
        assert_eq!(
            sip::header_value(&info_c, "Call-ID").as_deref(),
            Some(ims_call_c.as_str())
        );
        assert_eq!(sip::sip_body(&info_c), b"Signal=7\r\nDuration=200\r\n");

        let resume_rtp = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        handle_operator_command(
            &live,
            &runtime,
            OperatorCommand::Renegotiate {
                call_id: "matrix-call-a".into(),
                trunk_local_ip: "127.0.0.1".parse().unwrap(),
                offer: test_audio_offer(resume_rtp.local_addr().unwrap(), MediaDirection::SendRecv),
            },
        )
        .await
        .unwrap();
        let resume = recv_test_sip(&pcscf).await;
        assert_eq!(
            parse_audio_sdp(sip::sip_body(&resume)).unwrap().direction,
            MediaDirection::SendRecv
        );
        let resumed = sip::build_response(
            &resume,
            200,
            "OK",
            Some("network-a"),
            None,
            Some(answer_a.as_bytes()),
        );
        assert!(handle_operator_sip_frame(&live, &runtime, &resumed)
            .await
            .unwrap());
        assert!(recv_test_sip(&pcscf).await.starts_with(b"ACK "));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, .. } if call_id == "matrix-call-a"
        ));

        for (call_id, ims_call_id) in [
            ("matrix-call-a", ims_call_a.as_str()),
            ("matrix-call-c", ims_call_c.as_str()),
        ] {
            handle_operator_command(
                &live,
                &runtime,
                OperatorCommand::HangupCall {
                    call_id: call_id.into(),
                },
            )
            .await
            .unwrap();
            let bye = recv_test_sip(&pcscf).await;
            assert!(bye.starts_with(b"BYE "));
            assert_eq!(
                sip::header_value(&bye, "Call-ID").as_deref(),
                Some(ims_call_id)
            );
        }
        *live.session.lock().await = None;
    }

    #[tokio::test]
    async fn operator_answer_activates_relay_and_maps_dtmf_payload() {
        let operator_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n",
            internal_remote.local_addr().unwrap().port()
        );
        let internal_audio = parse_audio_sdp(internal_sdp.as_bytes()).unwrap();
        let pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let operator_local = pending.operator_local_addr().unwrap();
        let internal_local = pending.internal_local_addr().unwrap();
        let mut call = LiveVoiceCall {
            direction: LiveVoiceDirection::MobileOriginated,
            dialog: sip::DialogIds::fresh(),
            callee_uri: "sip:+8613800138000@ims.example;user=phone".into(),
            invite_branch: "z9hG4bKtest".into(),
            initial_invite: None,
            internal_offer: MediaOffer {
                audio_endpoint: internal_remote.local_addr().unwrap(),
                audio: internal_audio,
                video: None,
                dtmf: crate::services::trunk::bridge::DtmfCapabilities {
                    rtp_event: Some(RtpTelephoneEvent {
                        payload_type: 101,
                        clock_rate: 8000,
                        events: Some("0-16".into()),
                    }),
                    sip_info: true,
                    preferred: crate::services::trunk::bridge::DtmfSource::RtpEvent,
                },
            },
            operator_local,
            internal_local,
            pending_relay: Some(pending),
            active_relay: None,
            ip_answer_wait_armed: false,
            operator_answered: false,
            next_cseq: 2,
            media_metrics: None,
            pending_operator_reinvite: None,
            pending_asterisk_reinvite: false,
            pending_video_relay: None,
            active_video_relay: None,
            operator_video_local: None,
            internal_video_local: None,
            pending_media_rollback: None,
            renegotiation_deadline: None,
            early_answer: None,
            transfer: None,
            transfer_deadline: None,
        };
        let operator_sdp = format!(
            "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=fmtp:96 0-16\r\na=sendrecv\r\n",
            operator_remote.local_addr().unwrap().port()
        );
        let answer = prepare_operator_media(&mut call, operator_sdp.as_bytes()).unwrap();
        assert!(answer.contains(&format!("m=audio {} RTP/AVP 0 101", internal_local.port())));
        assert!(call.active_relay.is_some());
        let operator = OperatorLink::default();
        let mut events = operator.subscribe_events();
        let first_operator_rtp = arm_first_rtp_ip_answer(&mut call).unwrap();
        assert!(arm_first_rtp_ip_answer(&mut call).is_none());
        spawn_first_rtp_ip_answer(
            operator,
            "delayed-ip-answer".into(),
            answer.as_bytes().to_vec(),
            first_operator_rtp,
        );

        let packet = crate::connectivity::modems::ims::vowifi::voice::RtpPacket {
            payload_type: 96,
            marker: true,
            sequence: 1,
            timestamp: 160,
            ssrc: 7,
            payload: vec![5, 0, 0, 160],
        }
        .encode();
        operator_remote
            .send_to(&packet, operator_local)
            .await
            .unwrap();
        let mut received = [0u8; 256];
        let (len, _) = tokio::time::timeout(
            Duration::from_secs(1),
            internal_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, body }
                if call_id == "delayed-ip-answer" && body == answer.as_bytes()
        ));
        assert_eq!(received[1] & 0x7f, 101);
        assert_eq!(&received[2..len], &packet[2..]);
    }

    #[tokio::test]
    async fn asterisk_answer_activates_mt_relay_and_maps_dtmf_payload() {
        let operator_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let operator_sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=fmtp:96 0-16\r\na=sendrecv\r\n",
            operator_remote.local_addr().unwrap().port()
        );
        let operator_audio = parse_audio_sdp(operator_sdp.as_bytes()).unwrap();
        let pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let operator_local = pending.operator_local_addr().unwrap();
        let internal_local = pending.internal_local_addr().unwrap();
        let mut call = LiveVoiceCall {
            direction: LiveVoiceDirection::MobileTerminated,
            dialog: sip::DialogIds {
                call_id: "ims-mt-1".into(),
                local_tag: "local".into(),
                remote_tag: Some("remote".into()),
                cseq: 1,
            },
            callee_uri: "sip:+8613800138000@ims.example".into(),
            invite_branch: String::new(),
            initial_invite: Some(Vec::new()),
            internal_offer: MediaOffer {
                audio_endpoint: operator_remote.local_addr().unwrap(),
                audio: operator_audio,
                video: None,
                dtmf: DtmfCapabilities {
                    rtp_event: Some(RtpTelephoneEvent {
                        payload_type: 96,
                        clock_rate: 8000,
                        events: Some("0-16".into()),
                    }),
                    sip_info: true,
                    preferred: DtmfSource::RtpEvent,
                },
            },
            operator_local,
            internal_local,
            pending_relay: Some(pending),
            active_relay: None,
            ip_answer_wait_armed: false,
            operator_answered: false,
            next_cseq: 1,
            media_metrics: None,
            pending_operator_reinvite: None,
            pending_asterisk_reinvite: false,
            pending_video_relay: None,
            active_video_relay: None,
            operator_video_local: None,
            internal_video_local: None,
            pending_media_rollback: None,
            renegotiation_deadline: None,
            early_answer: None,
            transfer: None,
            transfer_deadline: None,
        };
        let internal_sdp = format!(
            "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n",
            internal_remote.local_addr().unwrap().port()
        );
        let answer = prepare_incoming_media(&mut call, internal_sdp.as_bytes()).unwrap();
        assert!(answer.contains(&format!("m=audio {} RTP/AVP 0 96", operator_local.port())));
        assert!(call.active_relay.is_some());

        let packet = crate::connectivity::modems::ims::vowifi::voice::RtpPacket {
            payload_type: 101,
            marker: true,
            sequence: 1,
            timestamp: 160,
            ssrc: 9,
            payload: vec![5, 0, 0, 160],
        }
        .encode();
        internal_remote
            .send_to(&packet, internal_local)
            .await
            .unwrap();
        let mut received = [0u8; 256];
        let (len, _) = tokio::time::timeout(
            Duration::from_secs(1),
            operator_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(received[1] & 0x7f, 96);
        assert_eq!(&received[2..len], &packet[2..]);
    }

    #[tokio::test]
    async fn rejected_video_reinvite_restores_confirmed_audio_relay() {
        let operator_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let internal_remote = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let confirmed_audio = parse_audio_sdp(
            format!(
                "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n",
                internal_remote.local_addr().unwrap().port()
            )
            .as_bytes(),
        )
        .unwrap();
        let confirmed_offer = MediaOffer {
            audio: confirmed_audio,
            audio_endpoint: internal_remote.local_addr().unwrap(),
            video: None,
            dtmf: DtmfCapabilities {
                rtp_event: None,
                sip_info: true,
                preferred: DtmfSource::SipInfo,
            },
        };
        let confirmed_pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let confirmed_operator_local = confirmed_pending.operator_local_addr().unwrap();
        let confirmed_internal_local = confirmed_pending.internal_local_addr().unwrap();
        let confirmed_relay = confirmed_pending.activate(
            operator_remote.local_addr().unwrap(),
            confirmed_offer.audio_endpoint,
            std::iter::empty::<PayloadTypeMapping>(),
        );
        let mut call = LiveVoiceCall {
            direction: LiveVoiceDirection::MobileOriginated,
            dialog: sip::DialogIds::fresh(),
            callee_uri: "sip:+601112023012@ims.example;user=phone".into(),
            invite_branch: "z9hG4bKtest".into(),
            initial_invite: None,
            internal_offer: confirmed_offer.clone(),
            operator_local: confirmed_operator_local,
            internal_local: confirmed_internal_local,
            pending_relay: None,
            active_relay: Some(confirmed_relay),
            ip_answer_wait_armed: false,
            operator_answered: true,
            next_cseq: 2,
            media_metrics: None,
            pending_operator_reinvite: None,
            pending_asterisk_reinvite: true,
            pending_video_relay: None,
            active_video_relay: None,
            operator_video_local: None,
            internal_video_local: None,
            pending_media_rollback: None,
            renegotiation_deadline: Some(Instant::now() + REINVITE_TIMEOUT),
            early_answer: None,
            transfer: None,
            transfer_deadline: None,
        };

        let upgraded_internal = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let upgraded_video = tokio::net::UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let mut upgraded_offer = confirmed_offer.clone();
        upgraded_offer.audio_endpoint = upgraded_internal.local_addr().unwrap();
        upgraded_offer.video = Some(VideoOffer {
            description: crate::connectivity::core::ims_video::build_video_offer(
                "h264",
                99,
                "packetization-mode=1;profile-level-id=42e01f",
                upgraded_video.local_addr().unwrap().port(),
            ),
            endpoint: upgraded_video.local_addr().unwrap(),
        });
        let upgraded_pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let upgraded_operator_local = upgraded_pending.operator_local_addr().unwrap();
        let upgraded_internal_local = upgraded_pending.internal_local_addr().unwrap();
        let upgraded_video_pending =
            PendingRtpRelay::bind("127.0.0.1".parse().unwrap(), "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
        let upgraded_operator_video_local = upgraded_video_pending.operator_local_addr().unwrap();
        let upgraded_internal_video_local = upgraded_video_pending.internal_local_addr().unwrap();
        call.stage_media_update(
            upgraded_offer,
            upgraded_pending,
            upgraded_operator_local,
            upgraded_internal_local,
            Some(upgraded_video_pending),
            Some(upgraded_operator_video_local),
            Some(upgraded_internal_video_local),
        );

        // Simulate a 488 response to the video upgrade.
        call.pending_asterisk_reinvite = false;
        call.renegotiation_deadline = None;
        call.rollback_media_update();
        assert_eq!(call.internal_offer, confirmed_offer);
        assert_eq!(call.operator_local, confirmed_operator_local);
        assert_eq!(call.internal_local, confirmed_internal_local);
        assert!(call.active_relay.is_some());
        assert!(call.pending_relay.is_none());
        assert!(call.active_video_relay.is_none());
        assert!(call.pending_video_relay.is_none());

        let packet = crate::connectivity::core::voice::RtpPacket {
            payload_type: 0,
            marker: false,
            sequence: 1,
            timestamp: 160,
            ssrc: 0x0506_0708,
            payload: vec![0xcc, 0xdd],
        }
        .encode();
        operator_remote
            .send_to(&packet, confirmed_operator_local)
            .await
            .unwrap();
        let mut received = [0u8; 256];
        let (len, _) = tokio::time::timeout(
            Duration::from_secs(1),
            internal_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&received[..len], packet.as_slice());
    }

    #[tokio::test]
    async fn explicit_unregister_reuses_register_dialog_and_requires_final_success() {
        let (live, runtime, pcscf) = test_voice_session().await;
        let (expected_call_id, expected_from_tag, expected_cseq) = {
            let sessions = live.session.lock().await;
            let session = sessions.as_ref().unwrap();
            (
                session.register_ids.call_id.clone(),
                session.register_ids.from_tag.clone(),
                session.next_register_cseq,
            )
        };
        let unregister_live = live.clone();
        let unregister_runtime = Arc::clone(&runtime);
        let unregister = tokio::spawn(async move {
            unregister_live_session(&unregister_live, &unregister_runtime).await
        });

        let mut request = vec![0u8; 65_535];
        let (len, peer) =
            tokio::time::timeout(Duration::from_secs(1), pcscf.recv_from(&mut request))
                .await
                .unwrap()
                .unwrap();
        request.truncate(len);
        assert!(request.starts_with(b"REGISTER "));
        assert_eq!(
            sip::header_value(&request, "Call-ID").as_deref(),
            Some(expected_call_id.as_str())
        );
        assert!(sip::header_value(&request, "From")
            .is_some_and(|value| value.contains(&format!(";tag={expected_from_tag}"))));
        assert_eq!(
            sip::header_value(&request, "CSeq").as_deref(),
            Some(format!("{expected_cseq} REGISTER").as_str())
        );
        assert_eq!(sip::header_value(&request, "Expires").as_deref(), Some("0"));
        assert!(sip::header_value(&request, "Contact").is_some());

        let accepted =
            sip::build_response(&request, 200, "OK", Some("network-register"), None, None);
        pcscf.send_to(&accepted, peer).await.unwrap();
        assert_eq!(unregister.await.unwrap(), UnregisterResult::Confirmed);
        *live.session.lock().await = None;
    }

    #[tokio::test]
    async fn shared_register_contract_covers_volte_exchange_shape() {
        crate::connectivity::core::register::contract::assert_register_contract(
            crate::connectivity::core::register::contract::AuthenticatedExchangeStyle::SharedDriver,
            ImsRegistrationAccess::Volte,
        )
        .await;
    }
}
