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
    cellular::modem_manager::ModemBinding,
    ims::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
        register::{run_register, RegisterAuthenticator},
        voice::{parse_audio_sdp, SdpAddrType, SdpAudioDescription},
        ImsError,
    },
    infra::config::{TrunkIncomingMode, TrunkIpConnectMode, VolteConfig, VolteIpFamilyPreference},
    infra::db::{Database, SmsMessage},
    notify::notification::NotificationSender,
    trunk::{
        bridge::{
            DtmfCapabilities, DtmfSource, MediaOffer, OperatorCommand, OperatorEvent,
            RtpTelephoneEvent, VideoOffer,
        },
        operator::OperatorLink,
    },
};

use super::{
    bearer::{
        configure_bearer_network, disconnect_bearer, disconnect_existing_ims_bearers,
        ensure_ims_bearer, route_pcscf, teardown_bearer_network, BearerConnection, BearerRequest,
    },
    channel::VolteSipChannel,
    data_path::probe_ims_ipv6,
    digest_aka,
    errors::{code, VolteError},
    identity,
    ipsec::{self, SecAgree, XfrmInstallPlan},
    pcscf::{discover_pcscf, discover_pcscf_via_at_with_context, pcscf_socket, ImsAtContextLease},
    rtp_relay::{ActiveRtpRelay, PayloadTypeMapping, PendingRtpRelay},
    runtime::{RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteStage},
    sip::{self, ImsIdentity, RequestIds},
    sms::{MtIngest, MtReassembler, TRANSPORT_TAG},
    vilte::{negotiate_video, parse_video_sdp},
};

const QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const REGISTER_EXPIRES: u32 = 3600;
const REGISTER_REFRESH_AFTER_SECS: u64 = 3300;
const FIXED_IMS_FAMILY_ORDER: VolteIpFamilyPreference = VolteIpFamilyPreference::Ipv4First;
const MM_MODEM_WAIT_ATTEMPTS: usize = 10;
const MM_MODEM_WAIT_DELAY: Duration = Duration::from_secs(2);

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
    pcscf: SocketAddr,
    ip_family: &'static str,
    xfrm_plan: Option<XfrmInstallPlan>,
    at_context: Option<ImsAtContextLease>,
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
    media_metrics: Option<Arc<crate::trunk::operator::OperatorMediaMetrics>>,
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
}

impl VolteRegisterAuthenticator {
    fn new(
        identity: ImsIdentity,
        ids: RequestIds,
        sip_instance: String,
        offered_security_binding: SecAgree,
        route: ImsRoute,
        device: VolteDeviceBinding,
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
        }
    }
}

impl RegisterAuthenticator<VolteSipChannel> for VolteRegisterAuthenticator {
    async fn prepare_authenticated_channel(
        &mut self,
        challenge_response: &[u8],
        channel: &mut VolteSipChannel,
    ) -> Result<(), ImsError> {
        let challenge = parse_digest_challenge(challenge_response).map_err(to_ims_error)?;
        let aka_challenge = digest_aka::decode_aka_nonce(&challenge.nonce).map_err(to_ims_error)?;
        let aid = identity::resolve_usim_aid(None);
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
        if let Some((selected, verify)) = security_server {
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
        let prepared = self
            .pending
            .take()
            .ok_or(ImsError::new("volte_register_auth_not_prepared"))?;
        let mut ids = self.ids.clone();
        ids.cseq = cseq;
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
    dedupe_enabled: bool,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
) -> Result<VolteRuntimeStatus, VolteError> {
    if !config.feature_enabled || !config.connection_enabled {
        return Err(VolteError::new(code::RUNTIME_NOT_RUNNING));
    }
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
        })
        .await;

    match connect_inner(runtime, generation, device).await {
        Ok(session) => {
            let mode = if session.xfrm_plan.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            let pcscf = session.pcscf.to_string();
            let data_path_mode = format!("dedicated_ims_bearer_{}", session.ip_family);
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
) -> Result<VolteLiveSession, VolteError> {
    let mut device = resolve_device_binding(device).await?;
    runtime
        .update(|state| state.stage = VolteStage::Identity)
        .await;
    let device_identity = load_device_identity(&device).await?;
    ensure_generation(runtime, generation)?;

    runtime
        .update(|state| state.stage = VolteStage::Pcscf)
        .await;
    disconnect_existing_ims_bearers(&device.modem_id).await?;
    let at_discovery =
        discover_pcscf_via_at_with_context(&device.modem_id, FIXED_IMS_FAMILY_ORDER).await?;
    let at_pcscf = at_discovery.candidates;
    let mut at_context = at_discovery.context;
    if let Err(error) = ensure_generation(runtime, generation) {
        if let Some(context) = at_context.take() {
            context.cleanup().await;
        }
        return Err(error);
    }
    device = match resolve_device_binding(&device).await {
        Ok(device) => device,
        Err(error) => {
            if let Some(context) = at_context.take() {
                context.cleanup().await;
            }
            return Err(error);
        }
    };

    if let Err(error) = probe_ims_ipv6(&device.qmi_device, at_discovery.cid).await {
        tracing::warn!(error = %error, "VoLTE IMS IPv6 WDS preflight failed; continuing to bearer");
    }

    runtime
        .update(|state| state.stage = VolteStage::Bearer)
        .await;
    let request = BearerRequest::default();
    let mut bearer = match ensure_ims_bearer(&device.modem_id, &request).await {
        Ok(bearer) => bearer,
        Err(error)
            if at_context.is_some() && should_retry_bearer_after_at_context_cleanup(&error) =>
        {
            tracing::warn!(
                error = %error,
                "VoLTE IMS bearer prefix unavailable with retained AT context; retrying after legacy context cleanup"
            );
            if let Some(context) = at_context.take() {
                context.cleanup().await;
            }
            device = resolve_device_binding(&device).await?;
            ensure_ims_bearer(&device.modem_id, &request).await?
        }
        Err(error) => {
            if let Some(context) = at_context.take() {
                context.cleanup().await;
            }
            return Err(error);
        }
    };
    for candidate in at_pcscf {
        if !bearer.settings.pcscf.contains(&candidate) {
            bearer.settings.pcscf.push(candidate);
        }
    }
    let result = async {
        configure_bearer_network(&bearer).await?;
        ensure_generation(runtime, generation)?;
        let local_addrs = bearer.settings.ordered_local_addrs(FIXED_IMS_FAMILY_ORDER);
        if local_addrs.is_empty() {
            return Err(VolteError::new(code::IP_SETTINGS_MISSING));
        }
        let mut last_error = None;
        for (index, local_addr) in local_addrs.iter().copied().enumerate() {
            ensure_generation(runtime, generation)?;
            match connect_family(runtime, &bearer, &device_identity, local_addr, &device).await {
                Ok(session) => return Ok(session),
                Err(error) if index + 1 < local_addrs.len() && should_try_next_family(&error) => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED)))
    }
    .await;
    if result.is_err() {
        teardown_bearer_network(&bearer).await;
        disconnect_bearer(&bearer.path).await;
        if let Some(context) = at_context.take() {
            context.cleanup().await;
        }
    }
    result.map(|mut session| {
        session.at_context = at_context;
        session
    })
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
            state.stage = VolteStage::RegisterIpsec;
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
        at_context: None,
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
    let refresh_at = tokio::time::Instant::now() + Duration::from_secs(REGISTER_REFRESH_AFTER_SECS);
    loop {
        if runtime.generation() != generation {
            break;
        }
        if tokio::time::Instant::now() >= refresh_at {
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Degraded;
                    state.last_error = Some("volte_register_refresh_due".to_string());
                    state.reconnect_count += 1;
                })
                .await;
            cleanup_live_session(&live).await;
            break;
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

async fn cleanup_live_session(live: &VolteLiveHandle) {
    live.operator.set_ready(false);
    let session = live.session.lock().await.take();
    if let Some(session) = session {
        if let Some(plan) = session.xfrm_plan.as_ref() {
            ipsec::uninstall_plan(plan);
        }
        teardown_bearer_network(&session.bearer).await;
        disconnect_bearer(&session.bearer.path).await;
        if let Some(context) = session.at_context {
            context.cleanup().await;
        }
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

    let deliver = crate::ims::sms_codec::parse_mt_rp_data(sip::sip_body(frame))
        .map_err(|_| VolteError::new("volte_mt_rp_data_invalid"))?;
    let rp_ack_body = crate::ims::sms_codec::build_network_rp_ack(deliver.rp_message_reference);
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
                let fingerprint = crate::orchestrator::message_fingerprint(
                    &crate::orchestrator::MessageFingerprintInput {
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
                crate::infra::db::beijing_sms_now_string()
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
        crate::access::volte::sms::build_mo_submissions(recipient, text, service_center).map_err(
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
            if let Some(uri) = crate::ims::sip_frame::uri_from_header_value(entry) {
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
    Ok(DeviceIdentity {
        ims: identity::derive_identity(&imsi, mcc, mnc),
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

        if let Ok(modem_list) = command_output("mmcli", &["-L", "--output-keyvalue"]).await {
            if let Some(current_modem_id) = primary_modem_id(&modem_list) {
                if let Ok(details) = command_output(
                    "mmcli",
                    &["-m", current_modem_id.as_str(), "--output-keyvalue"],
                )
                .await
                {
                    if modem_is_ready(&details) {
                        tracing::warn!(
                            requested_modem = %requested.modem_id,
                            current_modem = %current_modem_id,
                            "VoLTE modem object changed; using current ModemManager modem"
                        );
                        let mut resolved = requested.clone();
                        resolved.modem_id = current_modem_id;
                        return Ok(resolved);
                    }
                }
            }
        }

        if attempt + 1 < MM_MODEM_WAIT_ATTEMPTS {
            tokio::time::sleep(MM_MODEM_WAIT_DELAY).await;
        }
    }
    Err(VolteError::new(code::RUNTIME_MM_MODEM_WAIT_TIMEOUT))
}

fn primary_modem_id(output: &str) -> Option<String> {
    let path = key_value(output, "modem-list.value[1]")?;
    let modem_id = path.rsplit('/').next()?.trim();
    (!modem_id.is_empty() && modem_id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| modem_id.to_string())
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

fn should_try_next_family(error: &VolteError) -> bool {
    matches!(
        error.code(),
        code::RUNTIME_ALL_PCSCF_FAILED
            | code::PCSCF_FAMILY_MISMATCH
            | code::IPSEC_UDP_BIND_FAILED
            | code::REGISTER_INITIAL_UNEXPECTED_STATUS
            | code::COMMAND_FAILED
    )
}

fn should_retry_bearer_after_at_context_cleanup(error: &VolteError) -> bool {
    error.code() == code::RUNTIME_MM_BEARER_CONNECT_FAILED
        && error
            .detail()
            .is_some_and(|detail| detail.contains("prefix-unavailable"))
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
    fn parses_current_modem_id_after_modem_manager_reenumeration() {
        let list =
            "modem-list.length : 1\nmodem-list.value[1] : /org/freedesktop/ModemManager1/Modem/7\n";
        assert_eq!(primary_modem_id(list).as_deref(), Some("7"));
        assert_eq!(primary_modem_id("modem-list.length : 0"), None);
    }

    #[test]
    fn modem_readiness_waits_for_registration() {
        assert!(modem_is_ready("modem.generic.state : registered\n"));
        assert!(modem_is_ready("modem.generic.state : connected\n"));
        assert!(!modem_is_ready("modem.generic.state : enabling\n"));
        assert!(!modem_is_ready("modem.generic.state : disabled\n"));
    }

    #[test]
    fn retained_at_context_fallback_is_limited_to_ipv6_prefix_failure() {
        let prefix = VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "volte_command_failed:mmcli:prefix-unavailable",
        );
        assert!(should_retry_bearer_after_at_context_cleanup(&prefix));

        let generic = VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "volte_command_failed:mmcli:operation-failed",
        );
        assert!(!should_retry_bearer_after_at_context_cleanup(&generic));
        assert!(!should_retry_bearer_after_at_context_cleanup(
            &VolteError::new(code::RUNTIME_ALL_PCSCF_FAILED)
        ));
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
        assert!(should_try_next_family(&VolteError::new(
            code::RUNTIME_ALL_PCSCF_FAILED
        )));
        assert!(should_try_next_family(&VolteError::new(
            code::REGISTER_INITIAL_UNEXPECTED_STATUS
        )));
        assert!(!should_try_next_family(&VolteError::new(
            code::REGISTER_AUTH_UNEXPECTED_STATUS
        )));
        assert!(!should_try_next_family(&VolteError::new(
            code::USIM_AKA_FAILED
        )));
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
                dtmf: crate::trunk::bridge::DtmfCapabilities {
                    rtp_event: Some(RtpTelephoneEvent {
                        payload_type: 101,
                        clock_rate: 8000,
                        events: Some("0-16".into()),
                    }),
                    sip_info: true,
                    preferred: crate::trunk::bridge::DtmfSource::RtpEvent,
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

        let packet = crate::access::vowifi::voice::RtpPacket {
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

        let packet = crate::access::vowifi::voice::RtpPacket {
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
