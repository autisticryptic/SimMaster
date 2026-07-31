//! Live VoLTE IMS registration driver for the Qualcomm target.
//!
//! This layer wires the pure stage-B pieces together: ModemManager owns the
//! dedicated `ims` bearer, Linux owns IP routing/xfrm, the USIM owns AKA, and
//! the shared `ims::register` driver owns the SIP transaction sequence.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::Utc;
use tokio::{process::Command, sync::Mutex};

use crate::{
    hardware::cellular::modem_manager::ModemBinding,
    connectivity::core::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
        register::{run_register, RegisterAuthenticator},
        voice::{parse_audio_sdp, SdpAddrType, SdpAudioDescription},
        ImsError,
    },
    platform::config::{TrunkIncomingMode, TrunkIpConnectMode, VolteConfig, VolteIpFamilyPreference},
    platform::db::{Database, SmsMessage},
    services::notify::notification::NotificationSender,
    services::trunk::{
        bridge::{
            DtmfCapabilities, DtmfSource, MediaOffer, OperatorCommand, OperatorEvent,
            RtpTelephoneEvent, VideoOffer,
        },
        operator::OperatorLink,
    },
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
    rtp_relay::{ActiveRtpRelay, PayloadTypeMapping, PendingRtpRelay},
    runtime::{RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteStage},
    sip::{self, ImsIdentity, RequestIds},
    sms::{MtIngest, MtReassembler, TRANSPORT_TAG},
    vilte::{negotiate_video, parse_video_sdp},
};

const QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const REGISTER_EXPIRES: u32 = 3600;
const REGISTER_REFRESH_AFTER_SECS: u64 = 3300;
const MM_MODEM_WAIT_ATTEMPTS: usize = 10;
const MM_MODEM_WAIT_DELAY: Duration = Duration::from_secs(2);

fn native_ims_bearer_required(_data_slot_mode: DataSlotMode) -> bool {
    // beta2 runs the IMS WDS bearer on the secondary DATA6 endpoint
    // (`Native VoLTE secondary QMI IMS WDS bearer started`, volte.rs:1976), never
    // on the primary port — starting a second data session on the primary port is
    // what returns `(2,201) [internal] error`. So every mode drives the native
    // secondary-endpoint path; the primary port stays with ModemManager. There is
    // deliberately no fallback to the ModemManager IMS bearer: that path wedges
    // this baseband.
    true
}

static DEFAULT_LIVE_HANDLE: OnceLock<VolteLiveHandle> = OnceLock::new();

/// Device-specific inputs formerly hard-coded to modem 0, `/dev/wwan0qmi0`
/// and UIM slot 1. A distinct value is injected for every discovered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolteDeviceBinding {
    pub line_id: String,
    pub modem_id: String,
    pub qmi_device: String,
    pub uim_slot: u8,
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
        })
    }

    fn legacy_default() -> Self {
        Self {
            line_id: "legacy-primary".to_string(),
            // ModemManager object indexes are ephemeral across service restarts.
            // The legacy/global path is single-modem, so use ModemManager's
            // stable selector exactly as the reference runtime does.
            modem_id: "any".to_string(),
            qmi_device: "/dev/wwan0qmi0".to_string(),
            uim_slot: 1,
        }
    }
}

/// One independently owned protected SIP session/listener pair. The handle is
/// cloneable so its receive task and API callers coordinate only within the
/// same physical modem/SIM line.
#[derive(Clone)]
pub struct VolteLiveHandle {
    session: Arc<Mutex<Option<VolteLiveSession>>>,
    listener: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    operator: OperatorLink,
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
            listener: Arc::new(Mutex::new(None)),
            operator: OperatorLink::default(),
        }
    }

    pub fn legacy_shared() -> Self {
        default_live_handle().clone()
    }

    pub fn operator_link(&self) -> OperatorLink {
        self.operator.clone()
    }
}

fn default_live_handle() -> &'static VolteLiveHandle {
    DEFAULT_LIVE_HANDLE.get_or_init(VolteLiveHandle::new)
}

pub fn default_live_operator_link() -> OperatorLink {
    default_live_handle().operator_link()
}

struct VolteLiveSession {
    channel: VolteSipChannel,
    identity: ImsIdentity,
    service_route: Option<String>,
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
    device: VolteDeviceBinding,
    aka_aid: Vec<u8>,
    voice_calls: HashMap<String, LiveVoiceCall>,
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
    media_metrics: Option<Arc<crate::services::trunk::operator::OperatorMediaMetrics>>,
    pending_operator_reinvite: Option<Vec<u8>>,
    pending_asterisk_reinvite: bool,
    pending_video_relay: Option<PendingRtpRelay>,
    active_video_relay: Option<ActiveRtpRelay>,
    operator_video_local: Option<SocketAddr>,
    internal_video_local: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveVoiceDirection {
    MobileOriginated,
    MobileTerminated,
}

struct DeviceIdentity {
    ims: ImsIdentity,
    aka_aid: Vec<u8>,
    usim_aid: String,
    isim_aid: Option<String>,
    source: &'static str,
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
}

impl VolteRegisterAuthenticator {
    fn new(
        identity: ImsIdentity,
        ids: RequestIds,
        sip_instance: String,
        offered_security_binding: SecAgree,
        route: ImsRoute,
        device: VolteDeviceBinding,
        runtime: VolteRuntime,
        reuse_security: bool,
        aka_aid: Vec<u8>,
    ) -> Self {
        let offered_security = offered_security_binding.security_client_value();
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
        }
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

        let request_uri = format!("sip:{}", self.identity.home_domain);
        if let Some(auts) = aka.auts.as_deref() {
            self.pending = Some(PreparedAuth {
                authorization: digest_aka::build_resync_authorization_header(
                    &challenge,
                    &self.identity.private_user,
                    &request_uri,
                    auts,
                ),
                security_client: Some(self.offered_security.clone()),
                security_verify: None,
                require_sec_agree: true,
            });
            self.route = channel.route();
            return Ok(());
        }

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

        let security_server = sip::header_values(challenge_response, "Security-Server")
            .into_iter()
            .find_map(|value| {
                ipsec::parse_security_server(&value)
                    .ok()
                    .map(|sec| (sec, value))
            });
        if self.reuse_security {
            let security_verify = channel.security_verify().map(str::to_string);
            self.mode = if security_verify.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            self.pending = Some(PreparedAuth {
                authorization,
                security_client: None,
                security_verify: security_verify.clone(),
                require_sec_agree: security_verify.is_some(),
            });
            self.route = channel.route();
            return Ok(());
        }
        if let Some((selected, verify)) = security_server {
            self.runtime
                .update(|state| state.stage = VolteStage::Ipsec)
                .await;
            let route = channel.route();
            let plan = ipsec::build_install_plan(
                route.local_addr.ip(),
                route.pcscf_addr.ip(),
                &self.offered_security_binding,
                &selected,
                &aka.ik,
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
            self.pending = Some(PreparedAuth {
                authorization,
                security_client: Some(self.offered_security.clone()),
                security_verify: Some(verify),
                require_sec_agree: true,
            });
        } else {
            self.mode = RegistrationMode::Udp;
            self.pending = Some(PreparedAuth {
                authorization,
                security_client: None,
                security_verify: None,
                require_sec_agree: false,
            });
        }
        self.route = channel.route();
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
        Ok(sip::build_register_with_security_policy(
            &self.identity,
            &self.route,
            &ids,
            REGISTER_EXPIRES,
            Some(&prepared.authorization),
            prepared.security_client.as_deref(),
            prepared.security_verify.as_deref(),
            &self.sip_instance,
            prepared.require_sec_agree,
        ))
    }
}

/// Establish the dedicated IMS bearer and REGISTER session. This is serialized
/// by the runtime guard and is safe to call repeatedly.
pub async fn connect_live(
    runtime: &Arc<VolteRuntime>,
    config: &VolteConfig,
    dedupe_enabled: bool,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
) -> Result<VolteRuntimeStatus, VolteError> {
    connect_live_for_line(
        default_live_handle(),
        &VolteDeviceBinding::legacy_default(),
        runtime,
        config,
        None,
        false,
        DataSlotMode::PrimaryImsOnly,
        dedupe_enabled,
        database,
        notification_sender,
    )
    .await
}

pub async fn connect_live_for_line(
    live: &VolteLiveHandle,
    device: &VolteDeviceBinding,
    runtime: &Arc<VolteRuntime>,
    config: &VolteConfig,
    line_ip_families: Option<&[crate::platform::config::VolteIpFamily]>,
    allow_roaming: bool,
    data_slot_mode: DataSlotMode,
    dedupe_enabled: bool,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
) -> Result<VolteRuntimeStatus, VolteError> {
    // Per-line connection intent is authoritative: the caller has already
    // verified `profile.enabled && profile.volte_connection_enabled` for this
    // physical line. The legacy global `VolteConfig::connection_enabled`
    // (and `feature_enabled`/`sms_enabled`) are no longer consulted here; they
    // are retained only for backward-compatible config/API serialization.
    let _advance = runtime.advance_guard().await;
    if live.session.lock().await.is_some() {
        return Ok(runtime.status().await);
    }
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

    // A per-line ordered family list wins; an empty or absent list falls back to
    // the legacy global single-select preference so existing installs are
    // unchanged.
    let plan = match line_ip_families {
        Some(families) if !families.is_empty() => ImsConnectionPlan::from_families(families),
        _ => ImsConnectionPlan::from_preference(config.ip_family_preference),
    };

    match connect_inner(
        runtime,
        generation,
        device,
        plan,
        allow_roaming,
        data_slot_mode,
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
            live.operator.set_ready(config.voice_enabled);
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
    runtime: &VolteRuntime,
    generation: u64,
    device: &VolteDeviceBinding,
    plan: ImsConnectionPlan,
    allow_roaming: bool,
    data_slot_mode: DataSlotMode,
) -> Result<VolteLiveSession, VolteError> {
    // The canonical connection plan is built by the caller (per-line ordered
    // families when set, else the global preference). All four family-selection
    // consumers (AT probe order, bearer fallback, IPv6 preflight hint, SIP
    // local-address order) derive from this one object.
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
    let device_identity = load_device_identity(&device).await?;
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

    // beta2-aligned P-CSCF ordering: pre-activate a temporary IMS profile and
    // read its P-CSCF first, then start WDS/ModemManager with the same profile.
    // Bearer PCO, active-context CGCONTRDP, and IMS DNS remain fallbacks.
    // A matching connected IMS bearer is reused by `ensure_ims_bearer`; only a
    // stale or policy-mismatched object is recreated. Preserving the connected
    // bearer also preserves the PCO state used for P-CSCF discovery.
    ensure_generation(runtime, generation)?;
    device = resolve_device_binding(&device).await?;

    runtime
        .update(|state| state.stage = VolteStage::Bearer)
        .await;
    let mut prefetched_pcscf = Vec::new();
    let mut ims_profile_lease = None;
    let ims_profile = match prefetch_pcscf_from_ims_profile(&device.modem_id, &plan).await {
        Ok(prefetch) => {
            let cid = prefetch.lease.cid;
            prefetched_pcscf = prefetch.candidates;
            tracing::info!(
                cid,
                pcscf_count = prefetched_pcscf.len(),
                "Prepared beta2-style IMS profile and retained its AT context"
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
            match prepare_ims_profile_context(&device.modem_id, &plan).await {
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
    };
    let mut request = BearerRequest::ims(allow_roaming);
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

    // Establish the IMS bearer directly over QMI on this line's secondary
    // (DATA6) endpoint, matching beta2 ("Native VoLTE secondary QMI IMS WDS
    // bearer started", volte.rs:1976). The primary port stays with ModemManager;
    // starting a second data session there is what returned the (2,201) internal
    // error in the field. IP configuration and P-CSCF come from AT+CGCONTRDP, so
    // no reusable WDS client id is needed and the session is a single start.
    let mut native_bearer = None;
    let native_required = native_ims_bearer_required(data_slot_mode);
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
                            device.qmi_device,
                            established.netdev.interface,
                            established.netdev.method.as_str()
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
        match ensure_bearer_with_runtime(runtime, &device.modem_id, &request, &plan).await {
            Ok(bearer) => bearer,
            Err(error) => {
                if let Some(established) = native_bearer.take() {
                    native_bearer::release_native_ims_bearer(established).await;
                }
                disable_pcscf_reporting(&device.modem_id, pcscf_reporting_cid).await;
                cleanup_ims_profile_lease(ims_profile_lease.take()).await;
                return Err(error);
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
        match discover_pcscf_via_active_at_context(&device.modem_id, &plan).await {
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
            match connect_family(runtime, &bearer, &device_identity, local_addr, &device).await {
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
    if result.is_err() {
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
) -> Result<VolteLiveSession, VolteError> {
    runtime
        .update(|state| state.stage = VolteStage::Pcscf)
        .await;
    let pcscf = discover_pcscf(
        &bearer.settings,
        &device_identity.ims.home_domain,
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
    let mut channel =
        VolteSipChannel::bind(route, Some(&bearer.interface), None).map_err(map_channel_error)?;
    let receive_port = channel
        .reserve_security_receive_port()
        .map_err(map_channel_error)?;
    let ids = RequestIds::fresh(1);
    let sip_instance = new_sip_instance();
    let offered_binding = offered_security(channel.route().local_addr.port(), receive_port);
    let offered = offered_binding.security_client_value();
    let initial_authorization = digest_aka::build_initial_authorization_header(
        &device_identity.ims.private_user,
        &device_identity.ims.home_domain,
        &format!("sip:{}", device_identity.ims.home_domain),
    );
    let initial = sip::build_register_with_security_policy(
        &device_identity.ims,
        &channel.route(),
        &ids,
        REGISTER_EXPIRES,
        Some(&initial_authorization),
        Some(&offered),
        None,
        &sip_instance,
        true,
    );
    let mut authenticator = VolteRegisterAuthenticator::new(
        device_identity.ims.clone(),
        ids,
        sip_instance,
        offered_binding,
        channel.route(),
        device.clone(),
        runtime.clone(),
        false,
        device_identity.aka_aid.clone(),
    );
    let registration = match run_register(&mut channel, &initial, &mut authenticator).await {
        Ok(registration) => registration,
        Err(error) => {
            if let Some(plan) = authenticator.xfrm_plan.as_ref() {
                ipsec::uninstall_plan(plan);
            }
            return Err(map_register_error(error));
        }
    };
    let service_route = register_service_route(&registration.response);
    let associated_uri = register_associated_uri(&registration.response);
    let mut registered_identity = device_identity.ims.clone();
    if let Some(uri) = associated_uri.as_deref() {
        // The network-provided default public user identity is authoritative
        // after REGISTER.  In particular, operators commonly authenticate
        // with the IMSI-derived IMPU but require subsequent MESSAGE requests
        // to use the MSISDN-associated IMPU in From/P-Preferred-Identity.
        registered_identity.public_uri = uri.to_string();
    }
    tracing::info!(
        service_route_present = service_route.is_some(),
        associated_uri_present = associated_uri.is_some(),
        "VoLTE IMS registration routing identities captured"
    );
    if authenticator.mode == RegistrationMode::Udp {
        runtime
            .update(|state| state.stage = VolteStage::RegisterUdp)
            .await;
    }
    Ok(VolteLiveSession {
        channel,
        identity: registered_identity,
        service_route,
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
        register_ids: authenticator.ids,
        next_register_cseq: 2 + u32::from(registration.auth_rounds),
        sip_instance: authenticator.sip_instance,
        security_binding: authenticator.offered_security_binding,
        device: device.clone(),
        aka_aid: device_identity.aka_aid.clone(),
        voice_calls: HashMap::new(),
    })
}

/// Tear down only resources owned by the current VoLTE session.
pub async fn disconnect_live(runtime: &Arc<VolteRuntime>, reason: &str) -> VolteRuntimeStatus {
    disconnect_live_for_line(default_live_handle(), runtime, reason).await
}

pub async fn disconnect_live_for_line(
    live: &VolteLiveHandle,
    runtime: &Arc<VolteRuntime>,
    reason: &str,
) -> VolteRuntimeStatus {
    if let Some(listener) = live.listener.lock().await.take() {
        listener.abort();
    }
    cleanup_live_session(live).await;
    runtime.reset_runtime(reason).await;
    runtime.status().await
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
    let mut refresh_at =
        tokio::time::Instant::now() + Duration::from_secs(REGISTER_REFRESH_AFTER_SECS);
    // The native IMS bearer now runs single-shot on the secondary QMI endpoint
    // (beta2 alignment): there is no retained WDS client id to probe for packet
    // status, so bearer health is observed through the REGISTER refresh cycle
    // below rather than an independent WDS query.
    loop {
        if runtime.generation() != generation {
            break;
        }
        if tokio::time::Instant::now() >= refresh_at {
            let refresh_result = {
                let mut sessions = live.session.lock().await;
                match sessions.as_mut() {
                    Some(session) => refresh_live_registration(session, &runtime).await,
                    None => break,
                }
            };
            match refresh_result {
                Ok(()) => {
                    refresh_at = tokio::time::Instant::now()
                        + Duration::from_secs(REGISTER_REFRESH_AFTER_SECS);
                    continue;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "VoLTE REGISTER refresh failed; rebuilding session");
                    runtime
                        .update(|state| {
                            state.phase = VoltePhase::Degraded;
                            state.last_error =
                                Some(format!("volte_register_refresh_failed:{error}"));
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

async fn refresh_live_registration(
    session: &mut VolteLiveSession,
    runtime: &VolteRuntime,
) -> Result<(), VolteError> {
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
    let initial_authorization = digest_aka::build_initial_authorization_header(
        &session.identity.private_user,
        &session.identity.home_domain,
        &format!("sip:{}", session.identity.home_domain),
    );
    let initial = sip::build_register_with_security_policy(
        &session.identity,
        &session.channel.route(),
        &ids,
        REGISTER_EXPIRES,
        Some(&initial_authorization),
        None,
        security_verify.as_deref(),
        &session.sip_instance,
        require_sec_agree,
    );
    let mut authenticator = VolteRegisterAuthenticator::new(
        session.identity.clone(),
        ids.clone(),
        session.sip_instance.clone(),
        session.security_binding.clone(),
        session.channel.route(),
        session.device.clone(),
        runtime.clone(),
        true,
        session.aka_aid.clone(),
    );
    let registration = match run_register(&mut session.channel, &initial, &mut authenticator).await
    {
        Ok(registration) => registration,
        Err(error) => {
            let error = map_register_error(error);
            runtime
                .record_attempt(
                    VolteStage::RegisterRefresh,
                    Some(session.ip_family),
                    "failed",
                    Some(&error),
                    None,
                )
                .await;
            return Err(error);
        }
    };

    session.next_register_cseq = ids
        .cseq
        .saturating_add(u32::from(registration.auth_rounds))
        .saturating_add(1);
    if let Some(route) = register_service_route(&registration.response) {
        session.service_route = Some(route);
    }
    if let Some(uri) = register_associated_uri(&registration.response) {
        session.identity.public_uri = uri;
    }
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
    Ok(())
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
    let initial_call = matches!(command, OperatorCommand::StartCall { .. });
    let result = handle_operator_command_inner(live, runtime, command).await;
    if result.is_err() && initial_call {
        live.operator
            .send_event(OperatorEvent::Unavailable { call_id });
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
                    &call.dialog,
                    &call.callee_uri,
                    call.next_cseq,
                )
            } else {
                sip::build_cancel(
                    &session.identity,
                    &session.channel.route(),
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
                &call.dialog,
                &call.callee_uri,
                cseq,
                signal.digit,
                signal.duration_ms,
            )?
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
            call.internal_offer = offer.clone();
            call.pending_relay = Some(pending);
            call.operator_local = operator_local;
            call.internal_local = internal_local;
            call.pending_asterisk_reinvite = true;
            call.pending_video_relay = video_relay;
            call.operator_video_local = operator_video_local;
            call.internal_video_local = internal_video_local;
            let body = relay_media_sdp(&offer, call.operator_local, call.operator_video_local);
            sip::build_reinvite(
                &session.identity,
                &session.channel.route(),
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
            let request = call
                .pending_operator_reinvite
                .take()
                .ok_or_else(|| VolteError::new("volte_voice_reinvite_not_pending"))?;
            let answer = prepare_incoming_media(call, &body)?;
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
            call.pending_relay = None;
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
        | OperatorCommand::SendDtmf { call_id, .. } => call_id,
    }
}

fn normalize_operator_callee(callee: &str, home_domain: &str) -> Result<String, VolteError> {
    let without_scheme = callee
        .trim()
        .strip_prefix("sip:")
        .or_else(|| callee.trim().strip_prefix("tel:"))
        .unwrap_or(callee.trim());
    let user = without_scheme
        .split('@')
        .next()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default();
    let digits = user.strip_prefix('+').unwrap_or(user);
    if !(3..=20).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(VolteError::new("volte_voice_callee_invalid"));
    }
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
    let mut output = relay_audio_sdp(&offer.audio, offer.dtmf.rtp_event.as_ref(), audio_local);
    if let (Some(video), Some(local)) = (offer.video.as_ref(), video_local) {
        let mut description = video.description.clone();
        description.media_port = local.port();
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
        let operator_dtmf = parse_telephone_event(sip::sip_body(frame));
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
        call.internal_offer = offer;
        call.pending_relay = Some(pending);
        call.operator_local = operator_local;
        call.internal_local = internal_local;
        call.pending_video_relay = video_relay;
        call.operator_video_local = operator_video_local;
        call.internal_video_local = internal_video_local;
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
    if frame.starts_with(b"SIP/2.0 ") {
        let status = sip::parse_status(frame)?;
        let method = sip::header_value(frame, "CSeq")
            .and_then(|value| value.split_whitespace().nth(1).map(str::to_string));
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
                    Some(prepare_operator_media(call, sip::sip_body(frame))?)
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
                let answer = prepare_operator_media(call, sip::sip_body(frame));
                let first_operator_rtp =
                    if !is_asterisk_reinvite && !immediate_ip_connect && answer.is_ok() {
                        arm_first_rtp_ip_answer(call)
                    } else {
                        None
                    };
                call.pending_asterisk_reinvite = false;
                call.operator_answered = true;
                let ack = sip::build_ack(&identity, &route, &call.dialog, &call.callee_uri);
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
                call.pending_relay = None;
                call.pending_video_relay = None;
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
    let caller =
        sip::sip_header_uri(frame, "P-Asserted-Identity").unwrap_or_else(|| from_uri.clone());
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
    let operator_dtmf = parse_telephone_event(sip::sip_body(frame));
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
        },
    );
    live.operator.send_event(OperatorEvent::Incoming {
        call_id: ims_call_id,
        caller: normalize_incoming_caller(&caller),
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
    internal_answer.codecs = operator_audio
        .codecs
        .iter()
        .filter_map(|operator_codec| {
            call.internal_offer
                .audio
                .codecs
                .iter()
                .find(|internal| internal.codec == operator_codec.codec)
                .cloned()
        })
        .collect();
    if internal_answer.codecs.is_empty() {
        return Err(VolteError::new("volte_voice_no_common_codec"));
    }
    let operator_dtmf = parse_telephone_event(body);
    let internal_dtmf = call.internal_offer.dtmf.rtp_event.as_ref();
    let mut mappings = operator_audio
        .codecs
        .iter()
        .filter_map(|operator_codec| {
            let internal = call
                .internal_offer
                .audio
                .codecs
                .iter()
                .find(|codec| codec.codec == operator_codec.codec)?;
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
        call.active_relay = Some(pending.activate_with_metrics(
            operator_remote,
            call.internal_offer.audio_endpoint,
            mappings,
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
        trunk_video.media_port = internal_local.port();
        answer.push_str(&trunk_video.media_lines());
    } else {
        call.pending_video_relay = None;
    }
    Ok(answer)
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
    operator_answer.codecs = call
        .internal_offer
        .audio
        .codecs
        .iter()
        .filter(|operator| {
            internal_audio
                .codecs
                .iter()
                .any(|internal| internal.codec == operator.codec)
        })
        .cloned()
        .collect();
    if operator_answer.codecs.is_empty() {
        return Err(VolteError::new("volte_voice_no_common_codec"));
    }
    let operator_dtmf = call.internal_offer.dtmf.rtp_event.as_ref();
    let internal_dtmf = parse_telephone_event(body);
    let mut mappings = operator_answer
        .codecs
        .iter()
        .filter_map(|operator| {
            let internal = internal_audio
                .codecs
                .iter()
                .find(|codec| codec.codec == operator.codec)?;
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
        call.active_relay = Some(pending.activate_with_metrics(
            call.internal_offer.audio_endpoint,
            internal_remote,
            mappings,
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
        ims_video.media_port = operator_local.port();
        answer.push_str(&ims_video.media_lines());
    } else {
        call.pending_video_relay = None;
    }
    Ok(answer)
}

fn media_endpoint_for_video(
    audio: &SdpAudioDescription,
    video: &super::vilte::VideoMediaDescription,
) -> Result<SocketAddr, VolteError> {
    let ip = audio
        .connection_addr
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
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        480 => "Temporarily Unavailable",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
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

fn parse_telephone_event(body: &[u8]) -> Option<RtpTelephoneEvent> {
    let text = std::str::from_utf8(body).ok()?;
    let mut event = None;
    let mut fmtps = HashMap::new();
    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let mut fields = value.split_whitespace();
            let payload_type = fields.next()?.parse::<u8>().ok()?;
            let encoding = fields.next()?;
            let mut encoding_fields = encoding.split('/');
            if encoding_fields
                .next()?
                .eq_ignore_ascii_case("telephone-event")
            {
                let clock_rate = encoding_fields.next()?.parse::<u32>().ok()?;
                event = Some((payload_type, clock_rate));
            }
        } else if let Some(value) = line.strip_prefix("a=fmtp:") {
            let (payload_type, events) = value.split_once(char::is_whitespace)?;
            fmtps.insert(payload_type.parse::<u8>().ok()?, events.trim().to_string());
        }
    }
    let (payload_type, clock_rate) = event?;
    Some(RtpTelephoneEvent {
        payload_type,
        clock_rate,
        events: fmtps.remove(&payload_type),
    })
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
    let rp_ack_body = crate::connectivity::core::sms_codec::build_network_rp_ack(deliver.rp_message_reference);
    let rp_ack = {
        let sessions = live.session.lock().await;
        let session = sessions
            .as_ref()
            .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
        sip::build_rp_ack(
            &session.identity,
            &session.channel.route(),
            session.service_route.as_deref(),
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
                    .claim_sms_dedup(&fingerprint, TRANSPORT_TAG)
                    .map_err(|error| {
                        VolteError::with_detail("volte_sms_db_failed", error.to_string())
                    })?;
                if !claimed {
                    runtime.update(|state| state.duplicate_count += 1).await;
                    return Ok(());
                }
            }
            if database
                .sms_exists_by_pdu(&message.dedup_marker)
                .map_err(|error| {
                    VolteError::with_detail("volte_sms_db_failed", error.to_string())
                })?
            {
                runtime.update(|state| state.duplicate_count += 1).await;
                return Ok(());
            }
            let timestamp = if message.service_center_timestamp.trim().is_empty() {
                crate::platform::db::beijing_sms_now_string()
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

/// Send one single- or multipart MO SMS over the registered protected IMS
/// channel. Holding the session lock across each MESSAGE transaction prevents
/// the background MT listener from consuming the corresponding SIP response.
pub async fn send_live_sms(
    runtime: &Arc<VolteRuntime>,
    recipient: &str,
    text: &str,
    service_center: &str,
) -> Result<VolteSmsSendResult, VolteError> {
    send_live_sms_for_line(
        default_live_handle(),
        runtime,
        recipient,
        text,
        service_center,
    )
    .await
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
    let submissions =
        crate::connectivity::modems::softstack::volte::sms::build_mo_submissions(recipient, text, service_center).map_err(
            |error| VolteError::with_detail("volte_sms_encode_failed", error.to_string()),
        )?;
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
            session.service_route.as_deref(),
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
                service_route_present = session.service_route.is_some(),
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
    sip::header_value(response, "Service-Route").and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Return the network-selected default public user identity from REGISTER.
/// P-Associated-URI is an ordered list (possibly repeated across header lines),
/// so the first supported URI is the default identity for later requests.
fn register_associated_uri(response: &[u8]) -> Option<String> {
    for value in sip::header_values(response, "P-Associated-URI") {
        let mut remainder = value.as_str();
        while let Some(start) = remainder.find('<') {
            let after_start = &remainder[start + 1..];
            let Some(end) = after_start.find('>') else {
                break;
            };
            let uri = after_start[..end].trim();
            if supported_associated_uri(uri) {
                return Some(uri.to_string());
            }
            remainder = &after_start[end + 1..];
        }

        for entry in value.split(',') {
            if let Some(uri) = crate::connectivity::core::sip_frame::uri_from_header_value(entry) {
                let uri = uri.trim();
                if supported_associated_uri(uri) {
                    return Some(uri.to_string());
                }
            }
        }
    }
    None
}

fn supported_associated_uri(uri: &str) -> bool {
    uri.starts_with("sip:") || uri.starts_with("sips:") || uri.starts_with("tel:")
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

async fn load_device_identity(device: &VolteDeviceBinding) -> Result<DeviceIdentity, VolteError> {
    let modem = command_output(
        "mmcli",
        &["-m", device.modem_id.as_str(), "--output-keyvalue"],
    )
    .await?;
    let operator = key_value(&modem, "modem.3gpp.operator-code")
        .filter(|value| value.len() == 5 || value.len() == 6)
        .ok_or_else(|| VolteError::new(code::MM_IMSI_MISSING))?;
    let sim_path = key_value(&modem, "modem.generic.sim")
        .ok_or_else(|| VolteError::new(code::MM_IMSI_MISSING))?;
    let sim = command_output("mmcli", &["-i", &sim_path, "--output-keyvalue"]).await?;
    let imsi = key_value(&sim, "sim.properties.imsi")
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| VolteError::new(code::IMSI_MISSING))?;
    let (mcc, mnc) = operator.split_at(3);
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
    Ok(DeviceIdentity {
        ims: identity::derive_identity(&imsi, mcc, mnc),
        aka_aid,
        usim_aid,
        source: if isim_aid.is_some() {
            "imsi_fallback_isim_detected"
        } else {
            "imsi_fallback"
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

fn offered_security(send_port: u16, receive_port: u16) -> SecAgree {
    let spi = || {
        u32::from_str_radix(&sip::hex_token(4), 16)
            .ok()
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

    #[test]
    fn device_binding_uses_discovered_modem_qmi_and_slot() {
        let modem = ModemBinding {
            line_id: "line-0123456789abcdef0123456789abcdef".to_string(),
            modem_id: "7".to_string(),
            qmi_device: Some("/dev/cdc-wdm3".to_string()),
            uim_slot: 2,
            ..ModemBinding::default()
        };
        let device = VolteDeviceBinding::from_modem(&modem).unwrap();
        assert_eq!(device.modem_id, "7");
        assert_eq!(device.qmi_device, "/dev/cdc-wdm3");
        assert_eq!(device.uim_slot, 2);
    }

    #[test]
    fn device_binding_rejects_modem_without_qmi_control_port() {
        assert!(VolteDeviceBinding::from_modem(&ModemBinding::default()).is_err());
    }

    #[test]
    fn legacy_binding_uses_modem_manager_any_selector() {
        assert_eq!(VolteDeviceBinding::legacy_default().modem_id, "any");
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

        let packet = crate::connectivity::modems::softstack::vowifi::voice::RtpPacket {
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
        };
        let internal_sdp = format!(
            "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n",
            internal_remote.local_addr().unwrap().port()
        );
        let answer = prepare_incoming_media(&mut call, internal_sdp.as_bytes()).unwrap();
        assert!(answer.contains(&format!("m=audio {} RTP/AVP 0 96", operator_local.port())));
        assert!(call.active_relay.is_some());

        let packet = crate::connectivity::modems::softstack::vowifi::voice::RtpPacket {
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
}
