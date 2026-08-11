//! Event-driven VoWiFi voice adapter on the protected REGISTER TCP channel.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, RwLock,
    },
    time::{Duration, Instant},
};

use tokio::{
    sync::{mpsc, Mutex},
    task::JoinHandle,
};

use crate::{
    connectivity::{
        core::{
            access::ImsChannel,
            context::{ImsIdentity, ImsRoute},
            ims_video::{negotiate_video, parse_video_sdp},
            media::{ActiveRtpRelay, PayloadTypeMapping, PendingRtpRelay},
            registration::{ImsRegistrationAccess, RegisteredImsContext},
            sip_frame,
            sip_message::SipHeader,
            supplementary::{
                build_mwi_subscribe, classify_mwi_frame, MwiIncomingFrame, SubscribeIds,
            },
            voice::{parse_audio_sdp, SdpAddrType, SdpAudioDescription},
        },
        modems::ims::volte::sip,
    },
    platform::config::{TrunkIncomingMode, TrunkIpConnectMode},
    services::{
        supplementary::SupplementaryRuntime,
        trunk::{
            bridge::{
                parse_rtp_telephone_event, DtmfCapabilities, DtmfSource, MediaOffer,
                OperatorCommand, OperatorEvent, RtpTelephoneEvent, VideoOffer,
            },
            operator::OperatorLink,
        },
    },
};

#[cfg(test)]
use super::channel::EpdgSipChannel;
use super::channel::SipChannel;

const CHANNEL_POLL: Duration = Duration::from_millis(500);
const MWI_SUBSCRIBE_EXPIRES_SECONDS: u32 = 3600;

static HANDLES: OnceLock<RwLock<HashMap<String, Arc<VowifiOperatorHandle>>>> = OnceLock::new();

fn handles() -> &'static RwLock<HashMap<String, Arc<VowifiOperatorHandle>>> {
    HANDLES.get_or_init(|| RwLock::new(HashMap::new()))
}

struct InstalledTask {
    profile_id: &'static str,
    replacement_tx: mpsc::UnboundedSender<RegisteredChannel>,
    task: JoinHandle<()>,
}

struct VowifiOperatorHandle {
    link: OperatorLink,
    generation: AtomicU64,
    installed: Mutex<Option<InstalledTask>>,
    supplementary: RwLock<Option<Arc<SupplementaryRuntime>>>,
}

impl VowifiOperatorHandle {
    fn new() -> Self {
        Self {
            link: OperatorLink::default(),
            generation: AtomicU64::new(0),
            installed: Mutex::new(None),
            supplementary: RwLock::new(None),
        }
    }
}

pub fn bind_supplementary_for_line(line_id: &str, runtime: Arc<SupplementaryRuntime>) {
    let handle = handle_for_line(line_id);
    *handle
        .supplementary
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime);
}

#[derive(Clone)]
pub struct RegisteredVoiceContext {
    pub line_id: String,
    pub profile_id: &'static str,
    pub identity: ImsIdentity,
    pub route: ImsRoute,
    pub registration: RegisteredImsContext,
    pub security_verify: Option<String>,
    pub pani: String,
    pub user_agent: String,
    pub expires_at: Instant,
    pub tcp_keepalive_interval: Option<Duration>,
    pub options_ping_interval: Option<Duration>,
}

struct RegisteredChannel {
    context: RegisteredVoiceContext,
    channel: SipChannel,
}

pub fn operator_link_for_line(line_id: &str) -> OperatorLink {
    let existing = handles()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned();
    if let Some(handle) = existing {
        return handle.link.clone();
    }
    let mut registry = handles()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry
        .entry(line_id.to_string())
        .or_insert_with(|| Arc::new(VowifiOperatorHandle::new()))
        .link
        .clone()
}

fn handle_for_line(line_id: &str) -> Arc<VowifiOperatorHandle> {
    let _ = operator_link_for_line(line_id);
    handles()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned()
        .expect("VoWiFi operator handle must exist")
}

pub async fn install_registered_channel(context: RegisteredVoiceContext, channel: SipChannel) {
    let handle = handle_for_line(&context.line_id);
    let mut installed = handle.installed.lock().await;
    let mut registration = RegisteredChannel { context, channel };
    if let Some(current) = installed.as_ref() {
        if current.profile_id == registration.context.profile_id && !current.task.is_finished() {
            match current.replacement_tx.send(registration) {
                Ok(()) => return,
                Err(error) => registration = error.0,
            }
        }
    }
    if let Some(previous) = installed.take() {
        previous.task.abort();
    }
    let generation = handle.generation.fetch_add(1, Ordering::SeqCst) + 1;
    handle.link.set_ready(false);
    let link = handle.link.clone();
    let handle_task = Arc::clone(&handle);
    let profile_id = registration.context.profile_id;
    let supplementary = handle
        .supplementary
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let mut commands = link.subscribe_commands();
    let (replacement_tx, replacement_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let line_id = registration.context.line_id.clone();
        let cleanup_supplementary = supplementary.clone();
        let result = run_session(
            registration.context,
            registration.channel,
            link.clone(),
            &mut commands,
            replacement_rx,
            supplementary,
        )
        .await;
        if let Some(runtime) = cleanup_supplementary {
            runtime
                .clear_registration(ImsRegistrationAccess::Vowifi)
                .await;
        }
        if let Err(reason) = result {
            tracing::warn!(
                line_id,
                profile_id,
                reason,
                "VoWiFi operator channel stopped"
            );
        }
        if handle_task.generation.load(Ordering::SeqCst) == generation {
            link.set_ready(false);
        }
    });
    *installed = Some(InstalledTask {
        profile_id,
        replacement_tx,
        task,
    });
    handle.link.set_ready(true);
}

pub async fn disconnect_line(line_id: &str) {
    let handle = handles()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned();
    let Some(handle) = handle else {
        return;
    };
    handle.generation.fetch_add(1, Ordering::SeqCst);
    handle.link.set_ready(false);
    if let Some(installed) = handle.installed.lock().await.take() {
        installed.task.abort();
    };
    let supplementary = {
        handle
            .supplementary
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    };
    if let Some(runtime) = supplementary {
        runtime
            .clear_registration(ImsRegistrationAccess::Vowifi)
            .await;
    }
}

pub async fn disconnect_profile(line_id: &str, profile_id: &str) {
    let handle = handles()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id)
        .cloned();
    let Some(handle) = handle else {
        return;
    };
    let matches = handle
        .installed
        .lock()
        .await
        .as_ref()
        .is_some_and(|installed| installed.profile_id == profile_id);
    if matches {
        disconnect_line(line_id).await;
    }
}

struct VoiceSession {
    context: RegisteredVoiceContext,
    channel: SipChannel,
    calls: HashMap<String, VoiceCall>,
    next_tcp_keepalive: Option<Instant>,
    next_options_ping: Option<Instant>,
    next_options_cseq: u32,
    supplementary: Option<Arc<SupplementaryRuntime>>,
    mwi_subscription: Option<MwiSubscription>,
}

struct MwiSubscription {
    ids: SubscribeIds,
    refresh_at: Instant,
}

struct VoiceCall {
    dialog: sip::DialogIds,
    remote_uri: String,
    invite_branch: String,
    initial_invite: Option<Vec<u8>>,
    internal_offer: MediaOffer,
    operator_local: SocketAddr,
    internal_local: SocketAddr,
    pending_relay: Option<PendingRtpRelay>,
    active_relay: Option<ActiveRtpRelay>,
    pending_video_relay: Option<PendingRtpRelay>,
    active_video_relay: Option<ActiveRtpRelay>,
    operator_video_local: Option<SocketAddr>,
    internal_video_local: Option<SocketAddr>,
    next_cseq: u32,
    pending_network_reinvite: Option<Vec<u8>>,
    pending_trunk_reinvite: bool,
    operator_answered: bool,
}

async fn run_session(
    context: RegisteredVoiceContext,
    channel: SipChannel,
    link: OperatorLink,
    commands: &mut tokio::sync::broadcast::Receiver<OperatorCommand>,
    mut replacements: mpsc::UnboundedReceiver<RegisteredChannel>,
    supplementary: Option<Arc<SupplementaryRuntime>>,
) -> Result<(), String> {
    let mut session = VoiceSession {
        next_tcp_keepalive: next_interval(context.tcp_keepalive_interval),
        next_options_ping: next_interval(context.options_ping_interval),
        next_options_cseq: 1,
        context,
        channel,
        calls: HashMap::new(),
        supplementary,
        mwi_subscription: None,
    };
    start_mwi_subscription(&mut session).await;
    let mut pending_replacement = None;
    loop {
        if session.calls.is_empty() {
            if let Some(replacement) = pending_replacement.take() {
                activate_registration(&mut session, replacement);
                start_mwi_subscription(&mut session).await;
                link.set_ready(true);
            }
        }
        if Instant::now() >= session.context.expires_at {
            if session.calls.is_empty() {
                return Err("vowifi_registration_expired".into());
            }
            link.set_ready(false);
        }
        tokio::select! {
            Some(replacement) = replacements.recv() => {
                if session.calls.is_empty() {
                    activate_registration(&mut session, replacement);
                    start_mwi_subscription(&mut session).await;
                    link.set_ready(true);
                } else {
                    pending_replacement = Some(replacement);
                }
            },
            command = commands.recv() => match command {
                Ok(command) => handle_command(&mut session, &link, command).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::error!(skipped, "VoWiFi operator command receiver lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            frame = session.channel.recv_sip(CHANNEL_POLL) => match frame {
                Ok(frame) => handle_frame(&mut session, &link, &frame).await?,
                Err(error) if error.code() == "ims_channel_read_timeout" => {}
                Err(error) => {
                    end_active_calls(&mut session, &link);
                    if let Some(replacement) = pending_replacement.take() {
                        activate_registration(&mut session, replacement);
                        start_mwi_subscription(&mut session).await;
                        link.set_ready(true);
                    } else {
                        return Err(error.code().to_string());
                    }
                }
            },
            _ = wait_until(session.next_tcp_keepalive) => {
                session.channel.send_keepalive().await
                    .map_err(|error| error.code().to_string())?;
                session.next_tcp_keepalive = next_interval(session.context.tcp_keepalive_interval);
            },
            _ = wait_until(session.next_options_ping) => {
                let frame = build_options(&session.context, session.next_options_cseq);
                session.next_options_cseq = session.next_options_cseq.saturating_add(1);
                session.channel.send_sip(&frame).await
                    .map_err(|error| error.code().to_string())?;
                session.next_options_ping = next_interval(session.context.options_ping_interval);
            },
            _ = wait_until(session.mwi_subscription.as_ref().map(|state| state.refresh_at)) => {
                start_mwi_subscription(&mut session).await;
            }
        }
    }
    Ok(())
}

fn next_interval(interval: Option<Duration>) -> Option<Instant> {
    interval.map(|interval| Instant::now() + interval)
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

fn activate_registration(session: &mut VoiceSession, replacement: RegisteredChannel) {
    session.context = replacement.context;
    session.channel = replacement.channel;
    session.next_tcp_keepalive = next_interval(session.context.tcp_keepalive_interval);
    session.next_options_ping = next_interval(session.context.options_ping_interval);
    session.next_options_cseq = 1;
    session.mwi_subscription = None;
}

async fn start_mwi_subscription(session: &mut VoiceSession) {
    let Some(runtime) = session.supplementary.as_ref().cloned() else {
        return;
    };
    if session.mwi_subscription.is_some() {
        if !runtime
            .owns_mwi_subscription(ImsRegistrationAccess::Vowifi)
            .await
        {
            session.mwi_subscription = None;
            return;
        }
    } else {
        runtime
            .begin_mwi_subscription(ImsRegistrationAccess::Vowifi)
            .await;
    }
    let ids = match session.mwi_subscription.take() {
        Some(previous) => SubscribeIds {
            branch: sip::new_branch(),
            from_tag: previous.ids.from_tag,
            to_tag: previous.ids.to_tag,
            call_id: previous.ids.call_id,
            cseq: previous.ids.cseq.saturating_add(1),
        },
        None => {
            let request_ids = sip::RequestIds::fresh(1);
            SubscribeIds {
                branch: sip::new_branch(),
                from_tag: request_ids.from_tag,
                to_tag: None,
                call_id: request_ids.call_id,
                cseq: request_ids.cseq,
            }
        }
    };
    let mut access_headers = vec![SipHeader::new(
        "P-Access-Network-Info",
        &session.context.pani,
    )];
    if let Some(value) = session.context.security_verify.as_deref() {
        access_headers.push(SipHeader::new("Security-Verify", value));
    }
    let frame = build_mwi_subscribe(
        &session.context.identity,
        &session.context.route,
        &session.context.registration,
        &ids,
        MWI_SUBSCRIBE_EXPIRES_SECONDS,
        &session.context.user_agent,
        &access_headers,
    );
    match session.channel.send_sip(&frame).await {
        Ok(()) => {
            let refresh_seconds =
                (u64::from(MWI_SUBSCRIBE_EXPIRES_SECONDS).saturating_mul(11) / 12).max(1);
            session.mwi_subscription = Some(MwiSubscription {
                ids,
                refresh_at: Instant::now() + Duration::from_secs(refresh_seconds),
            });
        }
        Err(error) => {
            runtime
                .fail_mwi_subscription(ImsRegistrationAccess::Vowifi, error.code())
                .await;
            session.mwi_subscription = None;
        }
    }
}

fn end_active_calls(session: &mut VoiceSession, link: &OperatorLink) {
    for (call_id, _) in session.calls.drain() {
        link.send_event(OperatorEvent::Ended { call_id });
    }
}

async fn handle_command(session: &mut VoiceSession, link: &OperatorLink, command: OperatorCommand) {
    let call_id = command_call_id(&command).to_string();
    let start = matches!(&command, OperatorCommand::StartCall { .. });
    let renegotiate = matches!(&command, OperatorCommand::Renegotiate { .. });
    if let Err(reason) = handle_command_inner(session, link, command).await {
        tracing::warn!(call_id, reason, "VoWiFi operator command failed");
        if start {
            session.calls.remove(&call_id);
            link.send_event(OperatorEvent::Unavailable { call_id });
        } else if renegotiate {
            link.send_event(OperatorEvent::Rejected {
                call_id,
                status: 488,
            });
        }
    }
}

async fn handle_command_inner(
    session: &mut VoiceSession,
    link: &OperatorLink,
    command: OperatorCommand,
) -> Result<(), String> {
    let frame = match command {
        OperatorCommand::StartCall {
            call_id,
            callee,
            trunk_local_ip,
            offer,
            ..
        } => {
            if offer.video.is_some() && !link.video_enabled() {
                return Err("vowifi_video_feature_disabled".into());
            }
            if session.calls.contains_key(&call_id) {
                return Err("vowifi_call_duplicate".into());
            }
            let remote_uri = normalize_callee(&callee, &session.context.identity.home_domain)?;
            let pending =
                PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
                    .await
                    .map_err(|error| format!("vowifi_rtp_bind_failed:{error}"))?;
            let operator_local = pending
                .operator_local_addr()
                .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
            let internal_local = pending
                .internal_local_addr()
                .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
            let (video_relay, operator_video_local, internal_video_local) = if offer.video.is_some()
            {
                let relay =
                    PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
                        .await
                        .map_err(|error| format!("vowifi_video_rtp_bind_failed:{error}"))?;
                let operator_local = relay
                    .operator_local_addr()
                    .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
                let internal_local = relay
                    .internal_local_addr()
                    .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
                (Some(relay), Some(operator_local), Some(internal_local))
            } else {
                (None, None, None)
            };
            let body = relay_media_sdp(&offer, operator_local, operator_video_local);
            let dialog = sip::DialogIds::fresh();
            let frame = sip::build_invite_for_access(
                &session.context.identity,
                &session.context.route,
                session.context.registration.service_route.as_deref(),
                &dialog,
                &remote_uri,
                body.as_bytes(),
                session.context.security_verify.as_deref(),
                &session.context.pani,
                &session.context.user_agent,
            );
            let invite_branch =
                top_via_branch(&frame).ok_or_else(|| "vowifi_invite_branch_missing".to_string())?;
            session.calls.insert(
                call_id,
                VoiceCall {
                    next_cseq: dialog.cseq.saturating_add(1),
                    dialog,
                    remote_uri,
                    invite_branch,
                    initial_invite: None,
                    internal_offer: offer,
                    operator_local,
                    internal_local,
                    pending_relay: Some(pending),
                    active_relay: None,
                    pending_video_relay: video_relay,
                    active_video_relay: None,
                    operator_video_local,
                    internal_video_local,
                    pending_network_reinvite: None,
                    pending_trunk_reinvite: false,
                    operator_answered: false,
                },
            );
            frame
        }
        OperatorCommand::CancelCall { call_id } => {
            let call = session
                .calls
                .remove(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            sip::build_cancel(
                &session.context.identity,
                &session.context.route,
                session.context.registration.service_route.as_deref(),
                &call.dialog,
                &call.remote_uri,
                &call.invite_branch,
            )
        }
        OperatorCommand::HangupCall { call_id } => {
            let call = session
                .calls
                .remove(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            if call.dialog.remote_tag.is_some() {
                sip::build_bye(
                    &session.context.identity,
                    &session.context.route,
                    session.context.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.remote_uri,
                    call.next_cseq,
                )
            } else {
                sip::build_cancel(
                    &session.context.identity,
                    &session.context.route,
                    session.context.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.remote_uri,
                    &call.invite_branch,
                )
            }
        }
        OperatorCommand::SendDtmf { call_id, signal } => {
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let cseq = call.next_cseq;
            call.next_cseq = call.next_cseq.saturating_add(1);
            sip::build_dtmf_info_for_access(
                &session.context.identity,
                &session.context.route,
                session.context.registration.service_route.as_deref(),
                &call.dialog,
                &call.remote_uri,
                cseq,
                signal.digit,
                signal.duration_ms,
                &session.context.pani,
                &session.context.user_agent,
            )
            .map_err(|error| error.to_string())?
        }
        OperatorCommand::Renegotiate {
            call_id,
            trunk_local_ip,
            offer,
        } => {
            if offer.video.is_some() && !link.video_enabled() {
                return Err("vowifi_video_feature_disabled".into());
            }
            let pending =
                PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
                    .await
                    .map_err(|error| format!("vowifi_rtp_bind_failed:{error}"))?;
            let operator_local = pending
                .operator_local_addr()
                .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
            let internal_local = pending
                .internal_local_addr()
                .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
            let (video_relay, operator_video_local, internal_video_local) = if offer.video.is_some()
            {
                let relay =
                    PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
                        .await
                        .map_err(|error| format!("vowifi_video_rtp_bind_failed:{error}"))?;
                let operator_local = relay
                    .operator_local_addr()
                    .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
                let internal_local = relay
                    .internal_local_addr()
                    .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
                (Some(relay), Some(operator_local), Some(internal_local))
            } else {
                (None, None, None)
            };
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            if call.pending_network_reinvite.is_some() || call.pending_trunk_reinvite {
                return Err("vowifi_reinvite_pending".into());
            }
            call.dialog.cseq = call.next_cseq;
            call.next_cseq = call.next_cseq.saturating_add(1);
            call.internal_offer = offer.clone();
            call.pending_relay = Some(pending);
            call.operator_local = operator_local;
            call.internal_local = internal_local;
            call.pending_video_relay = video_relay;
            call.operator_video_local = operator_video_local;
            call.internal_video_local = internal_video_local;
            call.pending_trunk_reinvite = true;
            let body = relay_media_sdp(&offer, call.operator_local, call.operator_video_local);
            sip::build_reinvite_for_access(
                &session.context.identity,
                &session.context.route,
                session.context.registration.service_route.as_deref(),
                &call.dialog,
                &call.remote_uri,
                body.as_bytes(),
                session.context.security_verify.as_deref(),
                &session.context.pani,
                &session.context.user_agent,
            )
        }
        OperatorCommand::AcceptCall { call_id, body } => {
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let answer = prepare_incoming_media(call, &body, link)?;
            if call.operator_answered {
                return Ok(());
            }
            call.operator_answered = true;
            let request = call
                .initial_invite
                .as_deref()
                .ok_or_else(|| "vowifi_incoming_invite_missing".to_string())?;
            sip::build_response(
                request,
                200,
                "OK",
                Some(&call.dialog.local_tag),
                Some(&ims_contact(
                    &session.context.identity,
                    &session.context.route,
                )),
                Some(answer.as_bytes()),
            )
        }
        OperatorCommand::RejectCall { call_id, status } => {
            let call = session
                .calls
                .remove(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let request = call
                .initial_invite
                .as_deref()
                .ok_or_else(|| "vowifi_incoming_invite_missing".to_string())?;
            sip::build_response(
                request,
                status,
                sip_reason(status),
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
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let answer = body
                .as_deref()
                .map(|body| prepare_incoming_media(call, body, link))
                .transpose()?;
            let request = call
                .initial_invite
                .as_deref()
                .ok_or_else(|| "vowifi_incoming_invite_missing".to_string())?;
            sip::build_response(
                request,
                status,
                sip_reason(status),
                Some(&call.dialog.local_tag),
                Some(&ims_contact(
                    &session.context.identity,
                    &session.context.route,
                )),
                answer.as_deref().map(str::as_bytes),
            )
        }
        OperatorCommand::AcceptRenegotiation { call_id, body } => {
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let answer = prepare_incoming_media(call, &body, link)?;
            let request = call
                .pending_network_reinvite
                .take()
                .ok_or_else(|| "vowifi_network_reinvite_missing".to_string())?;
            sip::build_response(
                &request,
                200,
                "OK",
                Some(&call.dialog.local_tag),
                Some(&ims_contact(
                    &session.context.identity,
                    &session.context.route,
                )),
                Some(answer.as_bytes()),
            )
        }
        OperatorCommand::RejectRenegotiation { call_id, status } => {
            let call = session
                .calls
                .get_mut(&call_id)
                .ok_or_else(|| "vowifi_call_unknown".to_string())?;
            let request = call
                .pending_network_reinvite
                .take()
                .ok_or_else(|| "vowifi_network_reinvite_missing".to_string())?;
            call.pending_relay = None;
            call.pending_video_relay = None;
            sip::build_response(
                &request,
                status,
                sip_reason(status),
                Some(&call.dialog.local_tag),
                None,
                None,
            )
        }
    };
    session
        .channel
        .send_sip(&frame)
        .await
        .map_err(|error| error.code().to_string())
}

async fn handle_frame(
    session: &mut VoiceSession,
    link: &OperatorLink,
    frame: &[u8],
) -> Result<(), String> {
    let active_call_id = session
        .mwi_subscription
        .as_ref()
        .map(|subscription| subscription.ids.call_id.as_str());
    match classify_mwi_frame(frame, active_call_id) {
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
            session
                .channel
                .send_sip(&response)
                .await
                .map_err(|error| error.code().to_string())?;
            if let (Some(runtime), Some(summary)) = (session.supplementary.as_ref(), summary) {
                match summary {
                    Ok(summary) => {
                        runtime
                            .update_message_waiting(ImsRegistrationAccess::Vowifi, summary)
                            .await;
                    }
                    Err(error) => {
                        runtime
                            .fail_mwi_subscription(ImsRegistrationAccess::Vowifi, error.code())
                            .await;
                    }
                }
            }
            return Ok(());
        }
        MwiIncomingFrame::SubscribeResponse { status, to_tag } => {
            if let Some(runtime) = session.supplementary.as_ref() {
                match status {
                    Ok(200..=299) => {
                        if let (Some(subscription), Some(tag)) =
                            (session.mwi_subscription.as_mut(), to_tag)
                        {
                            subscription.ids.to_tag = Some(tag);
                        }
                        runtime
                            .mark_mwi_subscribed(ImsRegistrationAccess::Vowifi)
                            .await;
                    }
                    Ok(401 | 407) => {
                        runtime
                            .fail_mwi_subscription(
                                ImsRegistrationAccess::Vowifi,
                                "mwi_subscribe_authentication_required",
                            )
                            .await;
                    }
                    Ok(_) | Err(_) => {
                        runtime
                            .fail_mwi_subscription(
                                ImsRegistrationAccess::Vowifi,
                                "mwi_subscribe_rejected",
                            )
                            .await;
                    }
                }
            }
            return Ok(());
        }
        MwiIncomingFrame::Other => {}
    }
    let Some(ims_call_id) = sip_frame::header_value(frame, "Call-ID") else {
        return Ok(());
    };
    let trunk_call_id = session
        .calls
        .iter()
        .find(|(_, call)| call.dialog.call_id == ims_call_id)
        .map(|(call_id, _)| call_id.clone());

    if sip_frame::is_request(frame, "OPTIONS") || sip_frame::is_request(frame, "MESSAGE") {
        let response = sip::build_response(frame, 200, "OK", None, None, None);
        session
            .channel
            .send_sip(&response)
            .await
            .map_err(|error| error.code().to_string())?;
        return Ok(());
    }

    if sip_frame::is_request(frame, "INVITE") {
        return match trunk_call_id {
            Some(call_id) => begin_network_reinvite(session, link, &call_id, frame).await,
            None => begin_incoming_call(session, link, frame, ims_call_id).await,
        };
    }
    if let Some(call_id) = trunk_call_id {
        if sip_frame::is_request(frame, "BYE") {
            let response = sip::build_response(frame, 200, "OK", None, None, None);
            session
                .channel
                .send_sip(&response)
                .await
                .map_err(|error| error.code().to_string())?;
            session.calls.remove(&call_id);
            link.send_event(OperatorEvent::Ended { call_id });
            return Ok(());
        }
        if sip_frame::is_request(frame, "CANCEL") {
            let ok = sip::build_response(frame, 200, "OK", None, None, None);
            session
                .channel
                .send_sip(&ok)
                .await
                .map_err(|error| error.code().to_string())?;
            if let Some(call) = session.calls.remove(&call_id) {
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
                        .map_err(|error| error.code().to_string())?;
                }
            }
            link.send_event(OperatorEvent::Cancelled { call_id });
            return Ok(());
        }
        if sip_frame::is_request(frame, "INFO") {
            let response = sip::build_response(frame, 200, "OK", None, None, None);
            session
                .channel
                .send_sip(&response)
                .await
                .map_err(|error| error.code().to_string())?;
            if let Some(signal) = crate::services::trunk::bridge::parse_operator_dtmf_info(frame) {
                link.send_event(OperatorEvent::Dtmf { call_id, signal });
            }
            return Ok(());
        }
        if frame.starts_with(b"SIP/2.0 ") {
            return handle_response(session, link, &call_id, frame).await;
        }
    }
    Ok(())
}

async fn handle_response(
    session: &mut VoiceSession,
    link: &OperatorLink,
    call_id: &str,
    frame: &[u8],
) -> Result<(), String> {
    let status = sip_frame::parse_status(frame).map_err(|error| error.code().to_string())?;
    let cseq_method = sip_frame::header_value(frame, "CSeq")
        .and_then(|value| value.split_whitespace().nth(1).map(str::to_string))
        .unwrap_or_default();
    if !cseq_method.eq_ignore_ascii_case("INVITE") {
        return Ok(());
    }
    let call = session
        .calls
        .get_mut(call_id)
        .ok_or_else(|| "vowifi_call_unknown".to_string())?;
    if let Some(tag) = response_to_tag(frame) {
        call.dialog.set_remote_tag(tag);
    }
    if (100..200).contains(&status) {
        let body = (!sip_frame::body(frame).is_empty())
            .then(|| prepare_operator_media(call, sip_frame::body(frame), link))
            .transpose()?;
        if sip_frame::header_value(frame, "Require").is_some_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("100rel"))
        }) {
            if let Some(rseq) = sip_frame::header_value(frame, "RSeq")
                .and_then(|value| value.trim().parse::<u32>().ok())
            {
                let cseq = call.next_cseq;
                call.next_cseq = call.next_cseq.saturating_add(1);
                let prack = sip::build_prack(
                    &session.context.identity,
                    &session.context.route,
                    session.context.registration.service_route.as_deref(),
                    &call.dialog,
                    &call.remote_uri,
                    cseq,
                    rseq,
                    call.dialog.cseq,
                );
                session
                    .channel
                    .send_sip(&prack)
                    .await
                    .map_err(|error| error.code().to_string())?;
            }
        }
        link.send_event(OperatorEvent::Provisional {
            call_id: call_id.to_string(),
            status,
            body: body.map(String::into_bytes),
        });
        return Ok(());
    }
    if (200..300).contains(&status) {
        let answer = prepare_operator_media(call, sip_frame::body(frame), link)?;
        let ack = sip::build_ack(
            &session.context.identity,
            &session.context.route,
            session.context.registration.service_route.as_deref(),
            &call.dialog,
            &call.remote_uri,
        );
        session
            .channel
            .send_sip(&ack)
            .await
            .map_err(|error| error.code().to_string())?;
        let was_reinvite = call.pending_trunk_reinvite;
        call.operator_answered = true;
        call.pending_trunk_reinvite = false;
        if !was_reinvite && link.ip_connect_mode() == TrunkIpConnectMode::FirstRtp {
            if let Some(relay) = call.active_relay.as_ref() {
                let mut first_rtp = relay.subscribe_first_operator_rtp();
                let link = link.clone();
                let call_id = call_id.to_string();
                let body = answer.into_bytes();
                tokio::spawn(async move {
                    while !*first_rtp.borrow() {
                        if first_rtp.changed().await.is_err() {
                            return;
                        }
                    }
                    link.send_event(OperatorEvent::Answered { call_id, body });
                });
                return Ok(());
            }
        }
        link.send_event(OperatorEvent::Answered {
            call_id: call_id.to_string(),
            body: answer.into_bytes(),
        });
        return Ok(());
    }

    let was_reinvite = call.pending_trunk_reinvite;
    call.pending_trunk_reinvite = false;
    call.pending_relay = None;
    call.pending_video_relay = None;
    link.send_event(OperatorEvent::Rejected {
        call_id: call_id.to_string(),
        status,
    });
    if !was_reinvite {
        session.calls.remove(call_id);
    }
    Ok(())
}

async fn begin_incoming_call(
    session: &mut VoiceSession,
    link: &OperatorLink,
    frame: &[u8],
    ims_call_id: String,
) -> Result<(), String> {
    let Some(trunk_local_ip) = link.trunk_local_ip() else {
        return reject_request(session, frame, 480).await;
    };
    if !link.is_available() {
        return reject_request(session, frame, 480).await;
    }
    let operator_audio =
        parse_audio_sdp(sip_frame::body(frame)).map_err(|error| error.to_string())?;
    let operator_remote = media_endpoint(&operator_audio)?;
    let operator_video = parse_video_sdp(sip_frame::body(frame))
        .ok()
        .and_then(|description| {
            media_endpoint_for_video(&operator_audio, &description)
                .ok()
                .map(|endpoint| VideoOffer {
                    description,
                    endpoint,
                })
        });
    if operator_video.is_some() && !link.video_enabled() {
        return reject_request(session, frame, 488).await;
    }
    let operator_dtmf = parse_rtp_telephone_event(sip_frame::body(frame));
    let pending = PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
        .await
        .map_err(|error| format!("vowifi_rtp_bind_failed:{error}"))?;
    let operator_local = pending
        .operator_local_addr()
        .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
    let internal_local = pending
        .internal_local_addr()
        .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
    let (video_relay, operator_video_local, internal_video_local) = if operator_video.is_some() {
        let relay = PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
            .await
            .map_err(|error| format!("vowifi_video_rtp_bind_failed:{error}"))?;
        let operator_local = relay
            .operator_local_addr()
            .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
        let internal_local = relay
            .internal_local_addr()
            .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
        (Some(relay), Some(operator_local), Some(internal_local))
    } else {
        (None, None, None)
    };
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
    let remote_uri = header_uri(frame, "Contact")
        .or_else(|| header_uri(frame, "From"))
        .ok_or_else(|| "vowifi_incoming_caller_missing".to_string())?;
    let caller = crate::connectivity::core::supplementary::resolve_caller_identity(frame)
        .uri
        .as_deref()
        .map(normalize_caller)
        .unwrap_or_else(|| "sip:anonymous@anonymous.invalid".to_string());
    let remote_tag = header_tag(
        &sip_frame::header_value(frame, "From")
            .ok_or_else(|| "vowifi_incoming_from_missing".to_string())?,
    )
    .ok_or_else(|| "vowifi_incoming_from_tag_missing".to_string())?;
    let invite_cseq = cseq_number(frame, "INVITE")?;
    let dialog = sip::DialogIds {
        call_id: ims_call_id,
        local_tag: sip::hex_token(8),
        remote_tag: Some(remote_tag),
        cseq: invite_cseq,
    };
    let trying = sip::build_response(frame, 100, "Trying", Some(&dialog.local_tag), None, None);
    session
        .channel
        .send_sip(&trying)
        .await
        .map_err(|error| error.code().to_string())?;
    let operator_answered = link.incoming_mode() == TrunkIncomingMode::BoundImmediate;
    if operator_answered {
        let answer = relay_media_sdp(&offer, operator_local, operator_video_local);
        let accepted = sip::build_response(
            frame,
            200,
            "OK",
            Some(&dialog.local_tag),
            Some(&ims_contact(
                &session.context.identity,
                &session.context.route,
            )),
            Some(answer.as_bytes()),
        );
        session
            .channel
            .send_sip(&accepted)
            .await
            .map_err(|error| error.code().to_string())?;
    }
    let trunk_call_id = format!("vowifi-{}", sip::hex_token(12));
    let trunk_offer = relay_media_sdp(&offer, internal_local, internal_video_local);
    session.calls.insert(
        trunk_call_id.clone(),
        VoiceCall {
            next_cseq: invite_cseq.saturating_add(1),
            dialog,
            remote_uri,
            invite_branch: String::new(),
            initial_invite: Some(frame.to_vec()),
            internal_offer: offer,
            operator_local,
            internal_local,
            pending_relay: Some(pending),
            active_relay: None,
            pending_video_relay: video_relay,
            active_video_relay: None,
            operator_video_local,
            internal_video_local,
            pending_network_reinvite: None,
            pending_trunk_reinvite: false,
            operator_answered,
        },
    );
    link.send_event(OperatorEvent::Incoming {
        call_id: trunk_call_id,
        caller,
        body: trunk_offer.into_bytes(),
    });
    Ok(())
}

async fn begin_network_reinvite(
    session: &mut VoiceSession,
    link: &OperatorLink,
    call_id: &str,
    frame: &[u8],
) -> Result<(), String> {
    let Some(trunk_local_ip) = link.trunk_local_ip() else {
        return reject_request(session, frame, 480).await;
    };
    let call = session
        .calls
        .get_mut(call_id)
        .ok_or_else(|| "vowifi_call_unknown".to_string())?;
    if call.pending_network_reinvite.is_some() || call.pending_trunk_reinvite {
        return reject_request(session, frame, 491).await;
    }
    let operator_audio =
        parse_audio_sdp(sip_frame::body(frame)).map_err(|error| error.to_string())?;
    let operator_remote = media_endpoint(&operator_audio)?;
    let operator_video = parse_video_sdp(sip_frame::body(frame))
        .ok()
        .and_then(|description| {
            media_endpoint_for_video(&operator_audio, &description)
                .ok()
                .map(|endpoint| VideoOffer {
                    description,
                    endpoint,
                })
        });
    if operator_video.is_some() && !link.video_enabled() {
        return reject_request(session, frame, 488).await;
    }
    let operator_dtmf = parse_rtp_telephone_event(sip_frame::body(frame));
    let pending = PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
        .await
        .map_err(|error| format!("vowifi_rtp_bind_failed:{error}"))?;
    let operator_local = pending
        .operator_local_addr()
        .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
    let internal_local = pending
        .internal_local_addr()
        .map_err(|error| format!("vowifi_rtp_local_failed:{error}"))?;
    let (video_relay, operator_video_local, internal_video_local) = if operator_video.is_some() {
        let relay = PendingRtpRelay::bind(session.context.route.local_addr.ip(), trunk_local_ip)
            .await
            .map_err(|error| format!("vowifi_video_rtp_bind_failed:{error}"))?;
        let operator_local = relay
            .operator_local_addr()
            .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
        let internal_local = relay
            .internal_local_addr()
            .map_err(|error| format!("vowifi_video_rtp_local_failed:{error}"))?;
        (Some(relay), Some(operator_local), Some(internal_local))
    } else {
        (None, None, None)
    };
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
    call.internal_offer = offer;
    call.operator_local = operator_local;
    call.internal_local = internal_local;
    call.pending_relay = Some(pending);
    call.pending_video_relay = video_relay;
    call.operator_video_local = operator_video_local;
    call.internal_video_local = internal_video_local;
    call.pending_network_reinvite = Some(frame.to_vec());
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
        .map_err(|error| error.code().to_string())?;
    link.send_event(OperatorEvent::Renegotiate {
        call_id: call_id.to_string(),
        body: trunk_offer.into_bytes(),
    });
    Ok(())
}

async fn reject_request(
    session: &mut VoiceSession,
    frame: &[u8],
    status: u16,
) -> Result<(), String> {
    let response = sip::build_response(frame, status, sip_reason(status), None, None, None);
    session
        .channel
        .send_sip(&response)
        .await
        .map_err(|error| error.code().to_string())
}

fn prepare_operator_media(
    call: &mut VoiceCall,
    body: &[u8],
    link: &OperatorLink,
) -> Result<String, String> {
    let operator_audio = parse_audio_sdp(body).map_err(|error| error.to_string())?;
    let operator_remote = media_endpoint(&operator_audio)?;
    let mut internal_answer = operator_audio.clone();
    internal_answer.codecs = operator_audio
        .codecs
        .iter()
        .filter_map(|operator| {
            call.internal_offer
                .audio
                .find_matching_codec(operator)
                .cloned()
        })
        .collect();
    if internal_answer.codecs.is_empty() {
        return Err("vowifi_no_common_codec".into());
    }
    let operator_dtmf = parse_rtp_telephone_event(body);
    let mappings = payload_mappings(
        &operator_audio,
        &call.internal_offer.audio,
        operator_dtmf.as_ref(),
        call.internal_offer.dtmf.rtp_event.as_ref(),
    );
    if let Some(pending) = call.pending_relay.take() {
        call.active_relay = Some(pending.activate_with_metrics(
            operator_remote,
            call.internal_offer.audio_endpoint,
            mappings,
            Some(link.media_metrics()),
        ));
    }
    let mut answer = relay_audio_sdp(
        &internal_answer,
        call.internal_offer.dtmf.rtp_event.as_ref(),
        call.internal_local,
    );
    if let (Some(internal_video), Ok(operator_video), Some(_), Some(internal_local)) = (
        call.internal_offer.video.as_ref(),
        parse_video_sdp(body),
        call.operator_video_local,
        call.internal_video_local,
    ) {
        negotiate_video(&internal_video.description, &operator_video)
            .map_err(|error| format!("vowifi_video_negotiation_failed:{error}"))?;
        let operator_remote = media_endpoint_for_video(&operator_audio, &operator_video)?;
        if call.active_video_relay.is_none() || call.pending_video_relay.is_some() {
            let pending = call
                .pending_video_relay
                .take()
                .ok_or_else(|| "vowifi_video_rtp_relay_missing".to_string())?;
            let mappings = (operator_video.payload_type != internal_video.description.payload_type)
                .then_some(PayloadTypeMapping {
                    operator: operator_video.payload_type,
                    internal: internal_video.description.payload_type,
                });
            call.active_video_relay = Some(pending.activate_with_metrics(
                operator_remote,
                internal_video.endpoint,
                mappings,
                Some(link.media_metrics()),
            ));
        }
        let mut trunk_video = operator_video;
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

fn prepare_incoming_media(
    call: &mut VoiceCall,
    body: &[u8],
    link: &OperatorLink,
) -> Result<String, String> {
    let internal_audio = parse_audio_sdp(body).map_err(|error| error.to_string())?;
    let internal_remote = media_endpoint(&internal_audio)?;
    let mut operator_answer = call.internal_offer.audio.clone();
    operator_answer
        .codecs
        .retain(|operator| internal_audio.find_matching_codec(operator).is_some());
    if operator_answer.codecs.is_empty() {
        return Err("vowifi_no_common_codec".into());
    }
    let internal_dtmf = parse_rtp_telephone_event(body);
    let mappings = payload_mappings(
        &operator_answer,
        &internal_audio,
        call.internal_offer.dtmf.rtp_event.as_ref(),
        internal_dtmf.as_ref(),
    );
    if let Some(pending) = call.pending_relay.take() {
        call.active_relay = Some(pending.activate_with_metrics(
            call.internal_offer.audio_endpoint,
            internal_remote,
            mappings,
            Some(link.media_metrics()),
        ));
    }
    let mut answer = relay_audio_sdp(
        &operator_answer,
        call.internal_offer.dtmf.rtp_event.as_ref(),
        call.operator_local,
    );
    if let (Some(operator_video), Ok(internal_video), Some(operator_local)) = (
        call.internal_offer.video.as_ref(),
        parse_video_sdp(body),
        call.operator_video_local,
    ) {
        negotiate_video(&operator_video.description, &internal_video)
            .map_err(|error| format!("vowifi_video_negotiation_failed:{error}"))?;
        let internal_remote = media_endpoint_for_video(&internal_audio, &internal_video)?;
        if call.active_video_relay.is_none() || call.pending_video_relay.is_some() {
            let pending = call
                .pending_video_relay
                .take()
                .ok_or_else(|| "vowifi_video_rtp_relay_missing".to_string())?;
            let mappings = (operator_video.description.payload_type != internal_video.payload_type)
                .then_some(PayloadTypeMapping {
                    operator: operator_video.description.payload_type,
                    internal: internal_video.payload_type,
                });
            call.active_video_relay = Some(pending.activate_with_metrics(
                operator_video.endpoint,
                internal_remote,
                mappings,
                Some(link.media_metrics()),
            ));
        }
        let mut ims_video = operator_video.description.clone();
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

fn payload_mappings(
    operator: &SdpAudioDescription,
    internal: &SdpAudioDescription,
    operator_dtmf: Option<&RtpTelephoneEvent>,
    internal_dtmf: Option<&RtpTelephoneEvent>,
) -> Vec<PayloadTypeMapping> {
    let mut mappings = operator
        .codecs
        .iter()
        .filter_map(|operator| {
            let internal = internal.find_matching_codec(operator)?;
            (operator.payload_type != internal.payload_type).then_some(PayloadTypeMapping {
                operator: operator.payload_type,
                internal: internal.payload_type,
            })
        })
        .collect::<Vec<_>>();
    if let (Some(operator), Some(internal)) = (operator_dtmf, internal_dtmf) {
        if operator.payload_type != internal.payload_type {
            mappings.push(PayloadTypeMapping {
                operator: operator.payload_type,
                internal: internal.payload_type,
            });
        }
    }
    mappings
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

fn media_endpoint(audio: &SdpAudioDescription) -> Result<SocketAddr, String> {
    let ip = audio
        .connection_addr
        .parse::<IpAddr>()
        .map_err(|_| "vowifi_media_address_invalid".to_string())?;
    if audio.media_port == 0 {
        return Err("vowifi_media_port_invalid".into());
    }
    Ok(SocketAddr::new(ip, audio.media_port))
}

fn media_endpoint_for_video(
    audio: &SdpAudioDescription,
    video: &crate::connectivity::core::ims_video::VideoMediaDescription,
) -> Result<SocketAddr, String> {
    let ip = video
        .connection_addr
        .as_deref()
        .unwrap_or(&audio.connection_addr)
        .parse::<IpAddr>()
        .map_err(|_| "vowifi_video_address_invalid".to_string())?;
    if video.media_port == 0 {
        return Err("vowifi_video_port_invalid".into());
    }
    Ok(SocketAddr::new(ip, video.media_port))
}

fn normalize_callee(callee: &str, home_domain: &str) -> Result<String, String> {
    let without_scheme = callee
        .trim()
        .strip_prefix("sip:")
        .or_else(|| callee.trim().strip_prefix("tel:"))
        .unwrap_or(callee.trim());
    let raw_user = without_scheme
        .split('@')
        .next()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default();
    let user = raw_user
        .chars()
        .filter(|character| !character.is_whitespace() && !matches!(character, '-' | '(' | ')'))
        .collect::<String>();
    let digits = user.strip_prefix('+').unwrap_or(&user);
    if !(3..=20).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("vowifi_callee_invalid".into());
    }
    Ok(format!("sip:{user}@{home_domain};user=phone"))
}

fn normalize_caller(caller: &str) -> String {
    if let Some(number) = caller.strip_prefix("tel:") {
        format!("sip:{number}@simadmin")
    } else if let Some(uri) = caller.strip_prefix("sips:") {
        format!("sip:{uri}")
    } else {
        caller.to_string()
    }
}

fn build_options(context: &RegisteredVoiceContext, cseq: u32) -> Vec<u8> {
    use crate::connectivity::core::sip_message::{build_request, SipHeader, SipRequest};

    let ids = sip::RequestIds::fresh(cseq);
    let route = context
        .registration
        .service_route
        .clone()
        .unwrap_or_else(|| {
            let pcscf_host = sip::sip_host(context.route.pcscf_addr.ip());
            format!(
                "<sip:{pcscf_host}:{};transport={};lr>",
                context.route.pcscf_addr.port(),
                context.route.transport.as_param()
            )
        });
    let to = format!("<{}>", context.identity.public_uri);
    let mut headers = vec![
        SipHeader::new("Route", route),
        SipHeader::new(
            "P-Preferred-Identity",
            format!("<{}>", context.identity.public_uri),
        ),
        SipHeader::new("P-Access-Network-Info", &context.pani),
        SipHeader::new("Accept", "application/sdp"),
    ];
    if let Some(security_verify) = context.security_verify.as_deref() {
        headers.push(SipHeader::new("Security-Verify", security_verify));
    }
    headers.push(SipHeader::new("User-Agent", &context.user_agent));
    build_request(&SipRequest {
        method: "OPTIONS",
        request_uri: &context.identity.public_uri,
        route: context.route,
        branch: &sip::new_branch(),
        from_uri: &context.identity.public_uri,
        from_tag: &ids.from_tag,
        to_value: &to,
        call_id: &ids.call_id,
        cseq: ids.cseq,
        headers: &headers,
        body: &[],
    })
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

fn top_via_branch(frame: &[u8]) -> Option<String> {
    sip_frame::header_value(frame, "Via")?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("branch=").map(ToOwned::to_owned))
}

fn header_uri(frame: &[u8], name: &str) -> Option<String> {
    sip_frame::header_value(frame, name)
        .as_deref()
        .and_then(sip_frame::uri_from_header_value)
}

fn response_to_tag(frame: &[u8]) -> Option<String> {
    header_tag(&sip_frame::header_value(frame, "To")?)
}

fn header_tag(value: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.eq_ignore_ascii_case("tag")
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

fn cseq_number(frame: &[u8], expected: &str) -> Result<u32, String> {
    let value =
        sip_frame::header_value(frame, "CSeq").ok_or_else(|| "vowifi_cseq_missing".to_string())?;
    let mut parts = value.split_whitespace();
    let number = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| "vowifi_cseq_invalid".to_string())?;
    let method = parts
        .next()
        .ok_or_else(|| "vowifi_cseq_method_missing".to_string())?;
    if !method.eq_ignore_ascii_case(expected) {
        return Err("vowifi_cseq_method_mismatch".into());
    }
    Ok(number)
}

fn command_call_id(command: &OperatorCommand) -> &str {
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

fn sip_reason(status: u16) -> &'static str {
    match status {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        400 => "Bad Request",
        480 => "Temporarily Unavailable",
        481 => "Call/Transaction Does Not Exist",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        491 => "Request Pending",
        500 => "Server Internal Error",
        503 => "Service Unavailable",
        _ => "SIP Response",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        connectivity::core::context::SipTransport,
        services::trunk::bridge::{DtmfSignal, MediaOffer},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream, UdpSocket},
    };

    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    async fn read_frame(stream: &mut TcpStream, pending: &mut Vec<u8>) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(len) = sip_frame::complete_frame_len(pending) {
                    return pending.drain(..len).collect();
                }
                let mut chunk = [0u8; 2048];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0, "SIP test stream closed before a complete frame");
                pending.extend_from_slice(&chunk[..read]);
            }
        })
        .await
        .expect("SIP frame timed out")
    }

    fn response(request: &[u8], status: u16, reason: &str, tag: &str, body: &[u8]) -> Vec<u8> {
        let via = sip_frame::header_value(request, "Via").unwrap();
        let from = sip_frame::header_value(request, "From").unwrap();
        let mut to = sip_frame::header_value(request, "To").unwrap();
        if !to.to_ascii_lowercase().contains(";tag=") {
            to.push_str(";tag=");
            to.push_str(tag);
        }
        let call_id = sip_frame::header_value(request, "Call-ID").unwrap();
        let cseq = sip_frame::header_value(request, "CSeq").unwrap();
        let content_type = (!body.is_empty())
            .then_some("Content-Type: application/sdp\r\n")
            .unwrap_or_default();
        format!(
            "SIP/2.0 {status} {reason}\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to}\r\nCall-ID: {call_id}\r\nCSeq: {cseq}\r\n{content_type}Content-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body),
        )
        .into_bytes()
    }

    fn context(line_id: &str, client: &TcpStream, server: &TcpStream) -> RegisteredVoiceContext {
        RegisteredVoiceContext {
            line_id: line_id.to_string(),
            profile_id: "test-profile",
            identity: ImsIdentity {
                private_user: "001010123456789@ims.example".into(),
                public_uri: "sip:+601100000001@ims.example".into(),
                contact_user: "+601100000001".into(),
                home_domain: "ims.example".into(),
                contact_user_phone: true,
            },
            route: ImsRoute {
                local_addr: client.local_addr().unwrap(),
                pcscf_addr: server.local_addr().unwrap(),
                transport: SipTransport::Tcp,
            },
            registration: RegisteredImsContext::from_response(
                crate::connectivity::core::registration::ImsRegistrationAccess::Vowifi,
                b"SIP/2.0 200 OK\r\nService-Route: <sip:service-route.ims.example;lr>\r\nExpires: 30\r\n\r\n",
                30,
            ),
            security_verify: Some("ipsec-3gpp;alg=hmac-sha-1-96".into()),
            pani: "IEEE-802.11;utran-cell-id-3gpp=0010100000000000".into(),
            user_agent: "SimAdmin-VoWiFi-Test".into(),
            expires_at: Instant::now() + Duration::from_secs(30),
            tcp_keepalive_interval: None,
            options_ping_interval: None,
        }
    }

    fn audio_offer(endpoint: SocketAddr) -> MediaOffer {
        let sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n",
            endpoint.port()
        );
        MediaOffer {
            audio: parse_audio_sdp(sdp.as_bytes()).unwrap(),
            audio_endpoint: endpoint,
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

    #[test]
    fn relay_sdp_preserves_dialog_codecs_and_adds_dtmf() {
        let source = b"v=0\r\no=- 1 1 IN IP4 10.0.0.3\r\ns=-\r\nc=IN IP4 10.0.0.3\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
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
    }

    #[test]
    fn evs_payload_mappings_follow_fmtp_variant_and_dynamic_payload_type() {
        let operator = parse_audio_sdp(
            b"v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 109 110\r\na=rtpmap:109 EVS/16000\r\na=fmtp:109 br=5.9-24.4; bw=nb-swb\r\na=rtpmap:110 EVS/16000\r\na=fmtp:110 br=13.2; bw=swb\r\n",
        )
        .unwrap();
        let internal = parse_audio_sdp(
            b"v=0\r\no=- 2 2 IN IP4 192.0.2.2\r\ns=-\r\nc=IN IP4 192.0.2.2\r\nt=0 0\r\nm=audio 41000 RTP/AVP 121 120\r\na=rtpmap:121 EVS/16000\r\na=fmtp:121 br=13.2; bw=swb\r\na=rtpmap:120 EVS/16000\r\na=fmtp:120 br=5.9-24.4; bw=nb-swb\r\n",
        )
        .unwrap();

        let mappings = payload_mappings(&operator, &internal, None, None);
        assert!(mappings.contains(&PayloadTypeMapping {
            operator: 109,
            internal: 120,
        }));
        assert!(mappings.contains(&PayloadTypeMapping {
            operator: 110,
            internal: 121,
        }));
    }

    #[test]
    fn relay_sdp_keeps_audio_and_rewrites_video_endpoint() {
        let mut offer = audio_offer("127.0.0.1:32000".parse().unwrap());
        offer.video = Some(VideoOffer {
            description: crate::connectivity::core::ims_video::build_video_offer(
                "h264",
                99,
                "packetization-mode=1;profile-level-id=42e01f",
                50000,
            ),
            endpoint: "198.51.100.20:50000".parse().unwrap(),
        });

        let sdp = relay_media_sdp(
            &offer,
            "192.0.2.10:33000".parse().unwrap(),
            Some("192.0.2.11:33002".parse().unwrap()),
        );

        assert!(sdp.contains("m=audio 33000 RTP/AVP 0 101\r\n"));
        assert!(sdp.contains("m=video 33002 RTP/AVP 99\r\n"));
        assert!(sdp.contains("c=IN IP4 192.0.2.11\r\n"));
        assert!(sdp.contains("a=rtpmap:99 H264/90000\r\n"));
    }

    #[test]
    fn normalizes_real_device_number_without_losing_country_prefix() {
        assert_eq!(
            normalize_callee("+60 1112023012", "ims.example").unwrap(),
            "sip:+601112023012@ims.example;user=phone"
        );
    }

    #[tokio::test]
    async fn outgoing_dialog_keeps_tags_relays_media_and_forwards_dtmf() {
        let (client, mut server) = tcp_pair().await;
        let line_id = "operator-test-outgoing";
        let link = operator_link_for_line(line_id);
        link.set_trunk_local_ip(Some("127.0.0.1".parse().unwrap()));
        let mut events = link.subscribe_events();
        let route_context = context(line_id, &client, &server);
        install_registered_channel(
            route_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                client,
                Vec::new(),
                route_context.route,
                route_context.security_verify.clone(),
            )),
        )
        .await;
        assert!(link.is_available());

        let internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::StartCall {
            call_id: "trunk-call-a".into(),
            caller: "6108".into(),
            callee: "+60 1112023012".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(internal_rtp.local_addr().unwrap()),
        })
        .unwrap();

        let mut pending = Vec::new();
        let invite = read_frame(&mut server, &mut pending).await;
        assert!(invite.starts_with(b"INVITE sip:+601112023012@ims.example;user=phone SIP/2.0"));
        assert_eq!(
            sip_frame::header_value(&invite, "P-Access-Network-Info").as_deref(),
            Some("IEEE-802.11;utran-cell-id-3gpp=0010100000000000")
        );
        assert_eq!(
            sip_frame::header_value(&invite, "User-Agent").as_deref(),
            Some("SimAdmin-VoWiFi-Test")
        );
        assert_eq!(
            sip_frame::header_value(&invite, "Route").as_deref(),
            Some("<sip:service-route.ims.example;lr>")
        );
        let from_tag = header_tag(&sip_frame::header_value(&invite, "From").unwrap()).unwrap();
        let call_id = sip_frame::header_value(&invite, "Call-ID").unwrap();

        server
            .write_all(&response(&invite, 180, "Ringing", "network-a", &[]))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Provisional { call_id, status: 180, .. } if call_id == "trunk-call-a"
        ));

        let operator_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let answer = format!(
            "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=fmtp:96 0-16\r\na=sendrecv\r\n",
            operator_rtp.local_addr().unwrap().port()
        );
        server
            .write_all(&response(
                &invite,
                200,
                "OK",
                "network-a",
                answer.as_bytes(),
            ))
            .await
            .unwrap();
        let ack = read_frame(&mut server, &mut pending).await;
        assert!(ack.starts_with(b"ACK "));
        assert_eq!(
            sip_frame::header_value(&ack, "Route").as_deref(),
            Some("<sip:service-route.ims.example;lr>")
        );
        assert_eq!(sip_frame::header_value(&ack, "Call-ID").unwrap(), call_id);
        assert_eq!(
            header_tag(&sip_frame::header_value(&ack, "From").unwrap()).as_deref(),
            Some(from_tag.as_str())
        );
        assert_eq!(
            header_tag(&sip_frame::header_value(&ack, "To").unwrap()).as_deref(),
            Some("network-a")
        );
        let answered = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let OperatorEvent::Answered {
            call_id: answered_call_id,
            body,
        } = answered
        else {
            panic!("expected answered event");
        };
        assert_eq!(answered_call_id, "trunk-call-a");
        let trunk_answer = parse_audio_sdp(&body).unwrap();
        assert_ne!(
            trunk_answer.media_port,
            operator_rtp.local_addr().unwrap().port()
        );
        assert_eq!(
            trunk_answer.codecs[0].codec,
            crate::connectivity::core::voice::AudioCodec::Pcmu
        );

        let network_info_body = b"Signal=8\r\nDuration=180\r\n";
        let network_info = format!(
            "INFO sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKinfo-in\r\nFrom: <sip:+601112023012@ims.example>;tag=network-a\r\nTo: <sip:+601100000001@ims.example>;tag={}\r\nCall-ID: {}\r\nCSeq: 2 INFO\r\nContent-Type: application/dtmf-relay\r\nContent-Length: {}\r\n\r\n{}",
            from_tag,
            call_id,
            network_info_body.len(),
            String::from_utf8_lossy(network_info_body),
        );
        server.write_all(network_info.as_bytes()).await.unwrap();
        let info_ok = read_frame(&mut server, &mut pending).await;
        assert!(info_ok.starts_with(b"SIP/2.0 200 OK"));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Dtmf { call_id, signal }
                if call_id == "trunk-call-a" && signal.digit == '8' && signal.duration_ms == 180
        ));

        link.send_command(OperatorCommand::SendDtmf {
            call_id: "trunk-call-a".into(),
            signal: DtmfSignal {
                digit: '5',
                duration_ms: 240,
                source: DtmfSource::SipInfo,
            },
        })
        .unwrap();
        let info = read_frame(&mut server, &mut pending).await;
        assert!(info.starts_with(b"INFO "));
        assert_eq!(sip_frame::body(&info), b"Signal=5\r\nDuration=240\r\n");

        let renegotiated_internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::Renegotiate {
            call_id: "trunk-call-a".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(renegotiated_internal_rtp.local_addr().unwrap()),
        })
        .unwrap();
        let trunk_reinvite = read_frame(&mut server, &mut pending).await;
        assert!(trunk_reinvite.starts_with(b"INVITE "));
        assert_eq!(
            sip_frame::header_value(&trunk_reinvite, "Call-ID").as_deref(),
            Some(call_id.as_str())
        );
        assert_eq!(
            header_tag(&sip_frame::header_value(&trunk_reinvite, "From").unwrap()).as_deref(),
            Some(from_tag.as_str())
        );
        assert_eq!(
            header_tag(&sip_frame::header_value(&trunk_reinvite, "To").unwrap()).as_deref(),
            Some("network-a")
        );
        server
            .write_all(&response(
                &trunk_reinvite,
                200,
                "OK",
                "network-a",
                answer.as_bytes(),
            ))
            .await
            .unwrap();
        let reinvite_ack = read_frame(&mut server, &mut pending).await;
        assert!(reinvite_ack.starts_with(b"ACK "));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, .. } if call_id == "trunk-call-a"
        ));

        let network_reinvite_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let network_reinvite_offer = format!(
            "v=0\r\no=- 4 4 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=sendrecv\r\n",
            network_reinvite_rtp.local_addr().unwrap().port()
        );
        let network_reinvite = format!(
            "INVITE sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKnetwork-reinvite\r\nFrom: <sip:+601112023012@ims.example>;tag=network-a\r\nTo: <sip:+601100000001@ims.example>;tag={}\r\nContact: <sip:+601112023012@127.0.0.1:5060;transport=tcp>\r\nCall-ID: {}\r\nCSeq: 7 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            from_tag,
            call_id,
            network_reinvite_offer.len(),
            network_reinvite_offer,
        );
        server.write_all(network_reinvite.as_bytes()).await.unwrap();
        let network_trying = read_frame(&mut server, &mut pending).await;
        assert!(
            network_trying.starts_with(b"SIP/2.0 100 Trying"),
            "{}",
            String::from_utf8_lossy(&network_trying)
        );
        let network_change = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let OperatorEvent::Renegotiate {
            call_id: changed_call_id,
            body: changed_offer,
        } = network_change
        else {
            panic!("expected network renegotiation event");
        };
        assert_eq!(changed_call_id, "trunk-call-a");
        assert!(parse_audio_sdp(&changed_offer).is_ok());
        let changed_internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::AcceptRenegotiation {
            call_id: "trunk-call-a".into(),
            body: audio_offer(changed_internal_rtp.local_addr().unwrap())
                .audio
                .to_sdp()
                .into_bytes(),
        })
        .unwrap();
        let network_accepted = read_frame(&mut server, &mut pending).await;
        assert!(network_accepted.starts_with(b"SIP/2.0 200 OK"));
        assert!(parse_audio_sdp(sip_frame::body(&network_accepted)).is_ok());

        let rejected_internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::Renegotiate {
            call_id: "trunk-call-a".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(rejected_internal_rtp.local_addr().unwrap()),
        })
        .unwrap();
        let rejected_reinvite = read_frame(&mut server, &mut pending).await;
        assert!(rejected_reinvite.starts_with(b"INVITE "));
        server
            .write_all(&response(
                &rejected_reinvite,
                488,
                "Not Acceptable Here",
                "network-a",
                &[],
            ))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Rejected { call_id, status: 488 } if call_id == "trunk-call-a"
        ));

        link.send_command(OperatorCommand::HangupCall {
            call_id: "trunk-call-a".into(),
        })
        .unwrap();
        let bye = read_frame(&mut server, &mut pending).await;
        assert!(bye.starts_with(b"BYE "));
        disconnect_line(line_id).await;
    }

    #[tokio::test]
    async fn incoming_cancel_before_answer_terminates_the_invite() {
        let (client, mut server) = tcp_pair().await;
        let line_id = "operator-test-cancel";
        let link = operator_link_for_line(line_id);
        link.set_trunk_local_ip(Some("127.0.0.1".parse().unwrap()));
        let mut events = link.subscribe_events();
        let route_context = context(line_id, &client, &server);
        install_registered_channel(
            route_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                client,
                Vec::new(),
                route_context.route,
                route_context.security_verify.clone(),
            )),
        )
        .await;

        let operator_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let offer = audio_offer(operator_rtp.local_addr().unwrap())
            .audio
            .to_sdp();
        let invite = format!(
            "INVITE sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKcancelled\r\nFrom: <sip:+601112023012@ims.example>;tag=remote-cancel\r\nTo: <sip:+601100000001@ims.example>\r\nContact: <sip:+601112023012@127.0.0.1:5060;transport=tcp>\r\nCall-ID: ims-cancel-a\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            offer.len(),
            offer,
        );
        server.write_all(invite.as_bytes()).await.unwrap();
        let mut pending = Vec::new();
        assert!(read_frame(&mut server, &mut pending)
            .await
            .starts_with(b"SIP/2.0 100 Trying"));
        let incoming = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let OperatorEvent::Incoming { call_id, .. } = incoming else {
            panic!("expected incoming event");
        };

        let cancel = b"CANCEL sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKcancelled\r\nFrom: <sip:+601112023012@ims.example>;tag=remote-cancel\r\nTo: <sip:+601100000001@ims.example>\r\nCall-ID: ims-cancel-a\r\nCSeq: 1 CANCEL\r\nContent-Length: 0\r\n\r\n";
        server.write_all(cancel).await.unwrap();
        assert!(read_frame(&mut server, &mut pending)
            .await
            .starts_with(b"SIP/2.0 200 OK"));
        let terminated = read_frame(&mut server, &mut pending).await;
        assert!(terminated.starts_with(b"SIP/2.0 487 Request Terminated"));
        assert_eq!(
            sip_frame::header_value(&terminated, "CSeq").as_deref(),
            Some("1 INVITE")
        );
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Cancelled { call_id: cancelled } if cancelled == call_id
        ));
        disconnect_line(line_id).await;
    }

    #[tokio::test]
    async fn first_rtp_mode_delays_answer_until_operator_media_arrives() {
        let (client, mut server) = tcp_pair().await;
        let line_id = "operator-test-first-rtp";
        let link = operator_link_for_line(line_id);
        link.set_ip_connect_mode(TrunkIpConnectMode::FirstRtp);
        let mut events = link.subscribe_events();
        let route_context = context(line_id, &client, &server);
        install_registered_channel(
            route_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                client,
                Vec::new(),
                route_context.route,
                route_context.security_verify.clone(),
            )),
        )
        .await;

        let internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::StartCall {
            call_id: "first-rtp-call".into(),
            caller: "6108".into(),
            callee: "+601112023012".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(internal_rtp.local_addr().unwrap()),
        })
        .unwrap();
        let mut pending = Vec::new();
        let invite = read_frame(&mut server, &mut pending).await;
        let relay_offer = parse_audio_sdp(sip_frame::body(&invite)).unwrap();
        let relay_endpoint = media_endpoint(&relay_offer).unwrap();

        let operator_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let answer = audio_offer(operator_rtp.local_addr().unwrap())
            .audio
            .to_sdp();
        server
            .write_all(&response(
                &invite,
                200,
                "OK",
                "network-first",
                answer.as_bytes(),
            ))
            .await
            .unwrap();
        assert!(read_frame(&mut server, &mut pending)
            .await
            .starts_with(b"ACK "));
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));

        let rtp = [
            0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa0, 0x01, 0x02, 0x03, 0x04,
        ];
        operator_rtp.send_to(&rtp, relay_endpoint).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, .. } if call_id == "first-rtp-call"
        ));

        link.send_command(OperatorCommand::HangupCall {
            call_id: "first-rtp-call".into(),
        })
        .unwrap();
        assert!(read_frame(&mut server, &mut pending)
            .await
            .starts_with(b"BYE "));
        disconnect_line(line_id).await;
    }

    #[tokio::test]
    async fn registration_refresh_waits_for_active_dialog_before_handover() {
        let (first_client, mut first_server) = tcp_pair().await;
        let line_id = "operator-test-refresh";
        let link = operator_link_for_line(line_id);
        let mut events = link.subscribe_events();
        let first_context = context(line_id, &first_client, &first_server);
        install_registered_channel(
            first_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                first_client,
                Vec::new(),
                first_context.route,
                first_context.security_verify.clone(),
            )),
        )
        .await;

        let first_internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::StartCall {
            call_id: "refresh-call-a".into(),
            caller: "6108".into(),
            callee: "+601112023012".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(first_internal_rtp.local_addr().unwrap()),
        })
        .unwrap();
        let mut first_pending = Vec::new();
        let first_invite = read_frame(&mut first_server, &mut first_pending).await;
        let first_operator_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let first_answer = audio_offer(first_operator_rtp.local_addr().unwrap())
            .audio
            .to_sdp();
        first_server
            .write_all(&response(
                &first_invite,
                200,
                "OK",
                "network-refresh-a",
                first_answer.as_bytes(),
            ))
            .await
            .unwrap();
        assert!(read_frame(&mut first_server, &mut first_pending)
            .await
            .starts_with(b"ACK "));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Answered { call_id, .. } if call_id == "refresh-call-a"
        ));

        let (second_client, mut second_server) = tcp_pair().await;
        let second_context = context(line_id, &second_client, &second_server);
        install_registered_channel(
            second_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                second_client,
                Vec::new(),
                second_context.route,
                second_context.security_verify.clone(),
            )),
        )
        .await;

        link.send_command(OperatorCommand::SendDtmf {
            call_id: "refresh-call-a".into(),
            signal: DtmfSignal {
                digit: '3',
                duration_ms: 160,
                source: DtmfSource::SipInfo,
            },
        })
        .unwrap();
        assert!(read_frame(&mut first_server, &mut first_pending)
            .await
            .starts_with(b"INFO "));

        link.send_command(OperatorCommand::HangupCall {
            call_id: "refresh-call-a".into(),
        })
        .unwrap();
        assert!(read_frame(&mut first_server, &mut first_pending)
            .await
            .starts_with(b"BYE "));

        let second_internal_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        link.send_command(OperatorCommand::StartCall {
            call_id: "refresh-call-b".into(),
            caller: "6108".into(),
            callee: "+601112023012".into(),
            trunk_local_ip: "127.0.0.1".parse().unwrap(),
            offer: audio_offer(second_internal_rtp.local_addr().unwrap()),
        })
        .unwrap();
        let mut second_pending = Vec::new();
        let second_invite = read_frame(&mut second_server, &mut second_pending).await;
        assert!(second_invite.starts_with(b"INVITE "));
        assert_ne!(
            sip_frame::header_value(&second_invite, "Call-ID"),
            sip_frame::header_value(&first_invite, "Call-ID")
        );

        link.send_command(OperatorCommand::CancelCall {
            call_id: "refresh-call-b".into(),
        })
        .unwrap();
        assert!(read_frame(&mut second_server, &mut second_pending)
            .await
            .starts_with(b"CANCEL "));
        disconnect_line(line_id).await;
    }

    #[tokio::test]
    async fn profile_intervals_drive_tcp_keepalive_and_options_ping() {
        let (keepalive_client, mut keepalive_server) = tcp_pair().await;
        let keepalive_line = "operator-test-keepalive";
        let mut keepalive_context = context(keepalive_line, &keepalive_client, &keepalive_server);
        keepalive_context.tcp_keepalive_interval = Some(Duration::from_millis(10));
        install_registered_channel(
            keepalive_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                keepalive_client,
                Vec::new(),
                keepalive_context.route,
                keepalive_context.security_verify.clone(),
            )),
        )
        .await;
        let mut keepalive = [0u8; 4];
        tokio::time::timeout(
            Duration::from_secs(1),
            keepalive_server.read_exact(&mut keepalive),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&keepalive, b"\r\n\r\n");
        disconnect_line(keepalive_line).await;

        let (options_client, mut options_server) = tcp_pair().await;
        let options_line = "operator-test-options";
        let mut options_context = context(options_line, &options_client, &options_server);
        options_context.options_ping_interval = Some(Duration::from_millis(10));
        install_registered_channel(
            options_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                options_client,
                Vec::new(),
                options_context.route,
                options_context.security_verify.clone(),
            )),
        )
        .await;
        let mut pending = Vec::new();
        let options = read_frame(&mut options_server, &mut pending).await;
        assert!(options.starts_with(b"OPTIONS "));
        assert_eq!(
            sip_frame::header_value(&options, "P-Access-Network-Info").as_deref(),
            Some("IEEE-802.11;utran-cell-id-3gpp=0010100000000000")
        );
        assert_eq!(
            sip_frame::header_value(&options, "User-Agent").as_deref(),
            Some("SimAdmin-VoWiFi-Test")
        );
        disconnect_line(options_line).await;
    }

    #[tokio::test]
    async fn mwi_subscribe_and_notify_update_bound_line_runtime() {
        let (client, mut server) = tcp_pair().await;
        let line_id = "operator-test-mwi";
        let supplementary = Arc::new(SupplementaryRuntime::for_line(line_id));
        bind_supplementary_for_line(line_id, Arc::clone(&supplementary));
        let route_context = context(line_id, &client, &server);
        install_registered_channel(
            route_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                client,
                Vec::new(),
                route_context.route,
                route_context.security_verify.clone(),
            )),
        )
        .await;

        let mut pending = Vec::new();
        let subscribe = read_frame(&mut server, &mut pending).await;
        assert!(sip_frame::is_request(&subscribe, "SUBSCRIBE"));
        assert_eq!(
            sip_frame::header_value(&subscribe, "Route").as_deref(),
            Some("<sip:service-route.ims.example;lr>")
        );
        assert_eq!(
            sip_frame::header_value(&subscribe, "Event").as_deref(),
            Some("message-summary")
        );
        let subscribe_ok = response(&subscribe, 200, "OK", "mwi-network", &[]);
        server.write_all(&subscribe_ok).await.unwrap();

        let call_id = sip_frame::header_value(&subscribe, "Call-ID").unwrap();
        let body = "Messages-Waiting: yes\r\nMessage-Account: sip:mailbox@ims.example\r\nVoice-Message: 2/1 (1/0)\r\n";
        let notify = format!(
            "NOTIFY sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKmwi\r\nFrom: <sip:+601100000001@ims.example>;tag=mwi-network\r\nTo: <sip:+601100000001@ims.example>;tag=mwi-client\r\nCall-ID: {call_id}\r\nCSeq: 1 NOTIFY\r\nEvent: message-summary\r\nContent-Type: application/simple-message-summary\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        server.write_all(notify.as_bytes()).await.unwrap();
        let notify_ok = read_frame(&mut server, &mut pending).await;
        assert!(notify_ok.starts_with(b"SIP/2.0 200 OK"));

        let snapshot = supplementary.snapshot().await;
        assert!(snapshot.mwi_capability.ready);
        let summary = snapshot.message_waiting.unwrap();
        assert!(summary.messages_waiting);
        assert_eq!(summary.voice.unwrap().new, 2);
        disconnect_line(line_id).await;
    }

    #[tokio::test]
    async fn incoming_invite_round_trips_answer_and_remote_bye() {
        let (client, mut server) = tcp_pair().await;
        let line_id = "operator-test-incoming";
        let link = operator_link_for_line(line_id);
        link.set_trunk_local_ip(Some("127.0.0.1".parse().unwrap()));
        let mut events = link.subscribe_events();
        let route_context = context(line_id, &client, &server);
        install_registered_channel(
            route_context.clone(),
            SipChannel::Tcp(EpdgSipChannel::new(
                client,
                Vec::new(),
                route_context.route,
                route_context.security_verify.clone(),
            )),
        )
        .await;

        let operator_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let offer = format!(
            "v=0\r\no=- 3 3 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0 96\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\na=sendrecv\r\n",
            operator_rtp.local_addr().unwrap().port()
        );
        let invite = format!(
            "INVITE sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKincoming\r\nFrom: <sip:+601112023012@ims.example>;tag=remote-in\r\nTo: <sip:+601100000001@ims.example>\r\nContact: <sip:+601112023012@127.0.0.1:5060;transport=tcp>\r\nPrivacy: id\r\nP-Asserted-Identity: <tel:+601112023012>\r\nCall-ID: ims-incoming-a\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            offer.len(),
            offer,
        );
        server.write_all(invite.as_bytes()).await.unwrap();
        let mut pending = Vec::new();
        let trying = read_frame(&mut server, &mut pending).await;
        assert!(trying.starts_with(b"SIP/2.0 100 Trying"));
        let incoming = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let OperatorEvent::Incoming {
            call_id,
            caller,
            body,
        } = incoming
        else {
            panic!("expected incoming event");
        };
        assert_eq!(caller, "sip:anonymous@anonymous.invalid");
        assert!(call_id.starts_with("vowifi-"));
        let trunk_offer = parse_audio_sdp(&body).unwrap();
        assert_ne!(
            trunk_offer.media_port,
            operator_rtp.local_addr().unwrap().port()
        );

        let asterisk_rtp = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let answer = audio_offer(asterisk_rtp.local_addr().unwrap())
            .audio
            .to_sdp()
            .into_bytes();
        link.send_command(OperatorCommand::AcceptCall {
            call_id: call_id.clone(),
            body: answer,
        })
        .unwrap();
        let accepted = read_frame(&mut server, &mut pending).await;
        assert!(accepted.starts_with(b"SIP/2.0 200 OK"));
        assert!(header_tag(&sip_frame::header_value(&accepted, "To").unwrap()).is_some());

        let bye = format!(
            "BYE sip:+601100000001@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKbye\r\nFrom: <sip:+601112023012@ims.example>;tag=remote-in\r\nTo: <sip:+601100000001@ims.example>;tag=local\r\nCall-ID: ims-incoming-a\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n"
        );
        server.write_all(bye.as_bytes()).await.unwrap();
        let bye_ok = read_frame(&mut server, &mut pending).await;
        assert!(bye_ok.starts_with(b"SIP/2.0 200 OK"));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Ended { call_id: ended } if ended == call_id
        ));
        disconnect_line(line_id).await;
    }
}
