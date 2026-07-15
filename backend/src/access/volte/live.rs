//! Live VoLTE IMS registration driver for the Qualcomm target.
//!
//! This layer wires the pure stage-B pieces together: ModemManager owns the
//! dedicated `ims` bearer, Linux owns IP routing/xfrm, the USIM owns AKA, and
//! the shared `ims::register` driver owns the SIP transaction sequence.

use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, OnceLock},
    time::Duration,
};

use chrono::Utc;
use tokio::{process::Command, sync::Mutex};

use crate::{
    infra::db::{Database, SmsMessage},
    ims::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
        register::{run_register, RegisterAuthenticator},
        ImsError,
    },
    infra::config::VolteConfig,
    notify::notification::NotificationSender,
};

use super::{
    bearer::{
        configure_bearer_network, disconnect_bearer, disconnect_existing_ims_bearers,
        ensure_ims_bearer, route_pcscf, teardown_bearer_network, BearerConnection,
        BearerRequest,
    },
    channel::VolteSipChannel,
    digest_aka,
    errors::{code, VolteError},
    identity,
    ipsec::{self, SecAgree, XfrmInstallPlan},
    pcscf::{discover_pcscf, discover_pcscf_via_at, pcscf_socket},
    runtime::{RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteStage},
    sip::{self, ImsIdentity, RequestIds},
    sms::{MtIngest, MtReassembler, TRANSPORT_TAG},
};

const MODEM_ID: &str = "0";
const QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const QMI_DEVICE: &str = "/dev/wwan0qmi0";
const UIM_SLOT: u8 = 1;
const REGISTER_EXPIRES: u32 = 3600;
const REGISTER_REFRESH_AFTER_SECS: u64 = 3300;

static LIVE_SESSION: OnceLock<Mutex<Option<VolteLiveSession>>> = OnceLock::new();
static LIVE_LISTENER: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();

struct VolteLiveSession {
    channel: VolteSipChannel,
    identity: ImsIdentity,
    bearer: BearerConnection,
    pcscf: SocketAddr,
    ip_family: &'static str,
    xfrm_plan: Option<XfrmInstallPlan>,
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
}

impl VolteRegisterAuthenticator {
    fn new(
        identity: ImsIdentity,
        ids: RequestIds,
        sip_instance: String,
        offered_security_binding: SecAgree,
        route: ImsRoute,
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
        let aka = tokio::task::spawn_blocking(move || {
            identity::run_usim_aka(
                QMI_PROXY_SOCKET,
                QMI_DEVICE,
                UIM_SLOT,
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
            .find_map(|value| ipsec::parse_security_server(&value).ok().map(|sec| (sec, value)));
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
            let receive_local = SocketAddr::new(
                route.local_addr.ip(),
                self.offered_security_binding.port_s,
            );
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
    if !config.feature_enabled || !config.connection_enabled {
        return Err(VolteError::new(code::RUNTIME_NOT_RUNNING));
    }
    let _advance = runtime.advance_guard().await;
    if LIVE_SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .is_some()
    {
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

    match connect_inner(runtime, generation, config).await {
        Ok(session) => {
            let mode = if session.xfrm_plan.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            let pcscf = session.pcscf.to_string();
            let data_path_mode = format!("dedicated_ims_bearer_{}", session.ip_family);
            *LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await = Some(session);
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Registered;
                    state.stage = VolteStage::Registered;
                    state.registration_mode = mode;
                    state.pcscf = Some(pcscf);
                    state.registered_at = Some(now());
                    state.data_path_mode = Some(data_path_mode);
                })
                .await;
            start_live_listener(
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
    config: &VolteConfig,
) -> Result<VolteLiveSession, VolteError> {
    runtime.update(|state| state.stage = VolteStage::Identity).await;
    let device_identity = load_device_identity().await?;
    ensure_generation(runtime, generation)?;

    runtime.update(|state| state.stage = VolteStage::Pcscf).await;
    disconnect_existing_ims_bearers(MODEM_ID).await?;
    let at_pcscf = discover_pcscf_via_at(MODEM_ID, config.ip_family_preference).await;
    ensure_generation(runtime, generation)?;

    runtime.update(|state| state.stage = VolteStage::Bearer).await;
    let mut bearer = ensure_ims_bearer(MODEM_ID, &BearerRequest::default()).await?;
    for candidate in at_pcscf {
        if !bearer.settings.pcscf.contains(&candidate) {
            bearer.settings.pcscf.push(candidate);
        }
    }
    let result = async {
        configure_bearer_network(&bearer).await?;
        ensure_generation(runtime, generation)?;
        let local_addrs = bearer
            .settings
            .ordered_local_addrs(config.ip_family_preference);
        if local_addrs.is_empty() {
            return Err(VolteError::new(code::IP_SETTINGS_MISSING));
        }
        let mut last_error = None;
        for (index, local_addr) in local_addrs.iter().copied().enumerate() {
            ensure_generation(runtime, generation)?;
            match connect_family(runtime, &bearer, &device_identity, local_addr).await {
                Ok(session) => return Ok(session),
                Err(error)
                    if index + 1 < local_addrs.len() && should_try_next_family(&error) =>
                {
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
    }
    result
}

async fn connect_family(
    runtime: &VolteRuntime,
    bearer: &BearerConnection,
    device_identity: &DeviceIdentity,
    local_addr: IpAddr,
) -> Result<VolteLiveSession, VolteError> {
    runtime.update(|state| state.stage = VolteStage::Pcscf).await;
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
    let mut channel = VolteSipChannel::bind(route, Some(&bearer.interface), None)
        .map_err(map_channel_error)?;
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
    );
    let registration = run_register(&mut channel, &initial, &mut authenticator).await;
    if let Err(error) = registration {
        if let Some(plan) = authenticator.xfrm_plan.as_ref() {
            ipsec::uninstall_plan(plan);
        }
        return Err(map_register_error(error));
    }
    if authenticator.mode == RegistrationMode::Udp {
        runtime.update(|state| state.stage = VolteStage::RegisterUdp).await;
    }
    Ok(VolteLiveSession {
        channel,
        identity: device_identity.ims.clone(),
        bearer: bearer.clone(),
        pcscf: pcscf_socket(pcscf),
        ip_family: ip_family_name(local_addr),
        xfrm_plan: authenticator.xfrm_plan,
    })
}

/// Tear down only resources owned by the current VoLTE session.
pub async fn disconnect_live(runtime: &Arc<VolteRuntime>, reason: &str) -> VolteRuntimeStatus {
    if let Some(listener) = LIVE_LISTENER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .take()
    {
        listener.abort();
    }
    cleanup_live_session().await;
    runtime.reset_runtime(reason).await;
    runtime.status().await
}

async fn start_live_listener(
    runtime: Arc<VolteRuntime>,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
    generation: u64,
    dedupe_enabled: bool,
) {
    let mut listener = LIVE_LISTENER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await;
    if let Some(previous) = listener.take() {
        previous.abort();
    }
    *listener = Some(tokio::spawn(async move {
        live_receive_loop(
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
    runtime: Arc<VolteRuntime>,
    database: Arc<Database>,
    notification_sender: Arc<NotificationSender>,
    generation: u64,
    dedupe_enabled: bool,
) {
    let mut reassembler = MtReassembler::new();
    let refresh_at = tokio::time::Instant::now()
        + Duration::from_secs(REGISTER_REFRESH_AFTER_SECS);
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
            cleanup_live_session().await;
            break;
        }
        let frame = {
            let mut sessions = LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await;
            let Some(session) = sessions.as_mut() else {
                break;
            };
            match session.channel.recv_sip(Duration::from_secs(1)).await {
                Ok(frame) => frame,
                Err(error) if error.code() == "volte_channel_read_timeout" => continue,
                Err(error) => {
                    tracing::warn!(error = %error, "VoLTE protected SIP receive failed");
                    runtime
                        .update(|state| {
                            state.phase = VoltePhase::Degraded;
                            state.last_error = Some(error.to_string());
                            state.last_failure_at = Some(now());
                        })
                        .await;
                    drop(sessions);
                    cleanup_live_session().await;
                    break;
                }
            }
        };
        runtime
            .update(|state| state.last_rx_at = Some(now()))
            .await;
        if let Err(error) = handle_live_frame(
            &runtime,
            &database,
            &notification_sender,
            &mut reassembler,
            &frame,
            dedupe_enabled,
        )
        .await
        {
            tracing::warn!(error = %error, "VoLTE protected SIP frame handling failed");
        }
    }
}

async fn cleanup_live_session() {
    let session = LIVE_SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .take();
    if let Some(session) = session {
        if let Some(plan) = session.xfrm_plan.as_ref() {
            ipsec::uninstall_plan(plan);
        }
        teardown_bearer_network(&session.bearer).await;
        disconnect_bearer(&session.bearer.path).await;
    }
}

async fn handle_live_frame(
    runtime: &Arc<VolteRuntime>,
    database: &Arc<Database>,
    notification_sender: &Arc<NotificationSender>,
    reassembler: &mut MtReassembler,
    frame: &[u8],
    dedupe_enabled: bool,
) -> Result<(), VolteError> {
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
        send_live_frame(runtime, &response).await?;
        return Ok(());
    }

    // Complete the SIP transaction before parsing/storing the RP-DATA.
    let response = sip::build_response(frame, 200, "OK", None, None, None);
    send_live_frame(runtime, &response).await?;

    let deliver = crate::ims::sms_codec::parse_mt_rp_data(sip::sip_body(frame))
        .map_err(|_| VolteError::new("volte_mt_rp_data_invalid"))?;
    let rp_ack_body = crate::ims::sms_codec::build_network_rp_ack(deliver.rp_message_reference);
    let rp_ack = {
        let sessions = LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await;
        let session = sessions
            .as_ref()
            .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
        sip::build_rp_ack(
            &session.identity,
            &session.channel.route(),
            frame,
            &rp_ack_body,
            &session.identity.public_uri,
            session.channel.security_verify(),
        )
    };
    send_live_frame(runtime, &rp_ack).await?;
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
                .map_err(|error| VolteError::with_detail("volte_sms_db_failed", error.to_string()))?
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
                .insert_sms_at_with_transport(
                    "incoming",
                    &message.originator,
                    &message.text,
                    &timestamp,
                    "received",
                    Some(&message.dedup_marker),
                    TRANSPORT_TAG,
                )
                .map_err(|error| VolteError::with_detail("volte_sms_db_failed", error.to_string()))?;
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
            tracing::debug!(reference, have, total, "Buffered VoLTE MT multipart segment");
        }
        MtIngest::ParseError => return Err(VolteError::new("volte_mt_rp_data_invalid")),
    }
    Ok(())
}

async fn send_live_frame(
    runtime: &Arc<VolteRuntime>,
    frame: &[u8],
) -> Result<(), VolteError> {
    let mut sessions = LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await;
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
    if !runtime.status().await.registered {
        return Err(VolteError::new("volte_runtime_not_registered"));
    }
    if service_center.trim().is_empty() {
        return Err(VolteError::new("volte_smsc_missing"));
    }
    let submissions = crate::access::volte::sms::build_mo_submissions(
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
        let mut sessions = LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await;
        let session = sessions
            .as_mut()
            .ok_or_else(|| VolteError::new("volte_runtime_not_registered"))?;
        let service_center_uri = phone_uri(service_center, &session.identity.home_domain)?;
        let frame = sip::build_sms_message(
            &session.identity,
            &session.channel.route(),
            &service_center_uri,
            &service_center_uri,
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

fn phone_uri(number: &str, domain: &str) -> Result<String, VolteError> {
    let number = number.trim();
    if number.is_empty()
        || !number
            .chars()
            .enumerate()
            .all(|(index, character)| character.is_ascii_digit() || (index == 0 && character == '+'))
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

async fn load_device_identity() -> Result<DeviceIdentity, VolteError> {
    let modem = command_output("mmcli", &["-m", MODEM_ID, "--output-keyvalue"]).await?;
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
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!("{program}:{}", output.status.code().unwrap_or(-1)),
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

fn ip_family_name(address: IpAddr) -> &'static str {
    if address.is_ipv6() { "ipv6" } else { "ipv4" }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
