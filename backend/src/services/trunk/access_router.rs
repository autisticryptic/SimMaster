//! Policy-aware per-line routing between one Asterisk trunk and IMS access legs.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    platform::config::{AccessPathKind, VoicePathPolicy},
    services::orchestrator::voice_router::{plan_voice_route, VoiceLegReadiness},
};

use super::{
    bridge::{OperatorCommand, OperatorEvent},
    operator::{OperatorDiagnostics, OperatorLink},
};

#[derive(Clone)]
struct AccessBackend {
    kind: AccessPathKind,
    link: OperatorLink,
}

struct CallRoute {
    owner: AccessPathKind,
    remaining: Vec<AccessPathKind>,
    start: Option<OperatorCommand>,
}

/// Owns the public trunk-facing link and keeps each call pinned to exactly one
/// backend. This prevents two IMS stacks from consuming the same broadcast
/// command when VoLTE and VoWiFi are registered at the same time.
pub struct VoiceAccessRouter {
    trunk: OperatorLink,
    policy: Arc<RwLock<VoicePathPolicy>>,
    backends: Vec<AccessBackend>,
    task: Option<JoinHandle<()>>,
}

impl VoiceAccessRouter {
    pub fn new(policy: VoicePathPolicy, backends: Vec<(AccessPathKind, OperatorLink)>) -> Self {
        let trunk = OperatorLink::default();
        let policy = Arc::new(RwLock::new(policy.normalized()));
        let backends = backends
            .into_iter()
            .map(|(kind, link)| AccessBackend { kind, link })
            .collect::<Vec<_>>();

        let task = tokio::runtime::Handle::try_current().ok().map(|handle| {
            let command_rx = trunk.subscribe_commands();
            let event_receivers = backends
                .iter()
                .map(|backend| (backend.kind, backend.link.subscribe_events()))
                .collect();
            let trunk_task = trunk.clone();
            let policy_task = Arc::clone(&policy);
            let backends_task = backends.clone();
            handle.spawn(async move {
                run_router(
                    trunk_task,
                    policy_task,
                    backends_task,
                    command_rx,
                    event_receivers,
                )
                .await;
            })
        });

        Self {
            trunk,
            policy,
            backends,
            task,
        }
    }

    pub fn operator_link(&self) -> OperatorLink {
        self.trunk.clone()
    }

    pub fn set_policy(&self, policy: VoicePathPolicy) {
        *self
            .policy
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy.normalized();
    }

    pub fn set_backend_video_enabled(&self, kind: AccessPathKind, enabled: bool) {
        if let Some(backend) = backend(&self.backends, kind) {
            backend.link.set_video_enabled(enabled);
        }
        self.trunk.set_video_enabled(
            self.backends
                .iter()
                .any(|backend| backend.link.video_enabled()),
        );
    }
}

impl Drop for VoiceAccessRouter {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn run_router(
    trunk: OperatorLink,
    policy: Arc<RwLock<VoicePathPolicy>>,
    backends: Vec<AccessBackend>,
    mut commands: tokio::sync::broadcast::Receiver<OperatorCommand>,
    event_receivers: Vec<(
        AccessPathKind,
        tokio::sync::broadcast::Receiver<OperatorEvent>,
    )>,
) {
    let (event_tx, mut events) = mpsc::channel::<(AccessPathKind, OperatorEvent)>(64);
    let mut event_tasks = tokio::task::JoinSet::new();
    for (kind, mut receiver) in event_receivers {
        let sender = event_tx.clone();
        event_tasks.spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        if sender.send((kind, event)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            access = kind.as_str(),
                            skipped,
                            "Voice access event receiver lagged"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    drop(event_tx);

    let mut routes = HashMap::<String, CallRoute>::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        refresh_router_state(&trunk, &policy, &backends, &routes);
        tokio::select! {
            command = commands.recv() => match command {
                Ok(command) => route_command(command, &trunk, &policy, &backends, &mut routes),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::error!(skipped, "Trunk access command receiver lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            event = events.recv() => match event {
                Some((kind, event)) => route_event(kind, event, &trunk, &policy, &backends, &mut routes),
                None => break,
            },
            _ = ticker.tick() => {}
        }
    }

    trunk.set_ready(false);
    drop(event_tasks);
}

fn current_policy(policy: &RwLock<VoicePathPolicy>) -> VoicePathPolicy {
    policy
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn route_plan(
    policy: &VoicePathPolicy,
    backends: &[AccessBackend],
    video_required: bool,
) -> Vec<AccessPathKind> {
    let readiness = backends
        .iter()
        .map(|backend| {
            let available = backend.link.is_available()
                && (!video_required || backend.link.video_enabled());
            VoiceLegReadiness {
                kind: backend.kind,
                feature_enabled: true,
                registered: available,
                media_gateway_ready: available,
            }
        })
        .collect::<Vec<_>>();
    plan_voice_route(policy, &readiness).candidates
}

fn refresh_router_state(
    trunk: &OperatorLink,
    policy: &RwLock<VoicePathPolicy>,
    backends: &[AccessBackend],
    routes: &HashMap<String, CallRoute>,
) {
    let policy = current_policy(policy);
    let candidates = route_plan(&policy, backends, false);
    let owned_consumer = routes.values().any(|route| {
        backend(backends, route.owner).is_some_and(|backend| backend.link.has_command_consumer())
    });
    trunk.set_ready(!candidates.is_empty() || owned_consumer);

    let local_ip = trunk.trunk_local_ip();
    let incoming_mode = trunk.incoming_mode();
    let ip_connect_mode = trunk.ip_connect_mode();
    trunk.set_video_enabled(candidates.iter().any(|kind| {
        backend(backends, *kind).is_some_and(|backend| backend.link.video_enabled())
    }));
    let mut aggregate = OperatorDiagnostics::default();
    for backend in backends {
        backend.link.set_trunk_local_ip(local_ip);
        backend.link.set_incoming_mode(incoming_mode);
        backend.link.set_ip_connect_mode(ip_connect_mode);
        let diagnostics = backend.link.diagnostics();
        aggregate.active_relays = aggregate
            .active_relays
            .saturating_add(diagnostics.active_relays);
        aggregate.rtp_from_asterisk_packets = aggregate
            .rtp_from_asterisk_packets
            .saturating_add(diagnostics.rtp_from_asterisk_packets);
        aggregate.rtp_from_asterisk_bytes = aggregate
            .rtp_from_asterisk_bytes
            .saturating_add(diagnostics.rtp_from_asterisk_bytes);
        aggregate.rtp_to_asterisk_packets = aggregate
            .rtp_to_asterisk_packets
            .saturating_add(diagnostics.rtp_to_asterisk_packets);
        aggregate.rtp_to_asterisk_bytes = aggregate
            .rtp_to_asterisk_bytes
            .saturating_add(diagnostics.rtp_to_asterisk_bytes);
    }
    trunk.replace_relay_diagnostics(aggregate);
}

fn backend(backends: &[AccessBackend], kind: AccessPathKind) -> Option<&AccessBackend> {
    backends.iter().find(|backend| backend.kind == kind)
}

fn route_command(
    command: OperatorCommand,
    trunk: &OperatorLink,
    policy: &RwLock<VoicePathPolicy>,
    backends: &[AccessBackend],
    routes: &mut HashMap<String, CallRoute>,
) {
    let call_id = command_call_id(&command).to_string();
    if matches!(&command, OperatorCommand::StartCall { .. }) {
        let video_required = matches!(
            &command,
            OperatorCommand::StartCall { offer, .. } if offer.video.is_some()
        );
        let candidates = route_plan(&current_policy(policy), backends, video_required);
        let mut remaining = candidates.clone();
        while let Some(kind) = remaining.first().copied() {
            remaining.remove(0);
            let Some(selected) = backend(backends, kind) else {
                continue;
            };
            if selected.link.send_command(command.clone()).is_ok() {
                routes.insert(
                    call_id,
                    CallRoute {
                        owner: kind,
                        remaining,
                        start: Some(command),
                    },
                );
                return;
            }
        }
        trunk.send_event(OperatorEvent::Unavailable { call_id });
        return;
    }

    let Some(owner) = routes.get(&call_id).map(|route| route.owner) else {
        trunk.send_event(OperatorEvent::Unavailable { call_id });
        return;
    };
    let sent = backend(backends, owner)
        .is_some_and(|selected| selected.link.send_command(command.clone()).is_ok());
    if !sent {
        trunk.send_event(OperatorEvent::Unavailable {
            call_id: call_id.clone(),
        });
    }
    if is_terminal_command(&command) {
        routes.remove(&call_id);
    }
}

fn route_event(
    kind: AccessPathKind,
    event: OperatorEvent,
    trunk: &OperatorLink,
    policy: &RwLock<VoicePathPolicy>,
    backends: &[AccessBackend],
    routes: &mut HashMap<String, CallRoute>,
) {
    let call_id = event_call_id(&event).to_string();
    if matches!(&event, OperatorEvent::Incoming { .. }) {
        if let Some(route) = routes.get(&call_id) {
            if route.owner != kind {
                reject_incoming_collision(kind, &call_id, backends);
            }
            return;
        }
        let allowed = route_plan(&current_policy(policy), backends, false).contains(&kind);
        if !allowed {
            reject_incoming_collision(kind, &call_id, backends);
            return;
        }
        routes.insert(
            call_id,
            CallRoute {
                owner: kind,
                remaining: Vec::new(),
                start: None,
            },
        );
        trunk.send_event(event);
        return;
    }

    let Some(route) = routes.get_mut(&call_id) else {
        return;
    };
    if route.owner != kind {
        return;
    }

    if matches!(&event, OperatorEvent::Unavailable { .. }) {
        if let Some(start) = route.start.clone() {
            while let Some(next) = route.remaining.first().copied() {
                route.remaining.remove(0);
                let Some(selected) = backend(backends, next) else {
                    continue;
                };
                if selected.link.send_command(start.clone()).is_ok() {
                    route.owner = next;
                    tracing::warn!(call_id = %call_id, access = next.as_str(), "Voice call failed over to next access leg");
                    return;
                }
            }
        }
    }

    if matches!(&event, OperatorEvent::Answered { .. }) {
        route.start = None;
        route.remaining.clear();
    }
    let terminal = is_terminal_event(&event);
    trunk.send_event(event);
    if terminal {
        routes.remove(&call_id);
    }
}

fn reject_incoming_collision(kind: AccessPathKind, call_id: &str, backends: &[AccessBackend]) {
    if let Some(selected) = backend(backends, kind) {
        let _ = selected.link.send_command(OperatorCommand::RejectCall {
            call_id: call_id.to_string(),
            status: 480,
        });
    }
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

fn event_call_id(event: &OperatorEvent) -> &str {
    match event {
        OperatorEvent::Incoming { call_id, .. }
        | OperatorEvent::Provisional { call_id, .. }
        | OperatorEvent::Answered { call_id, .. }
        | OperatorEvent::Renegotiate { call_id, .. }
        | OperatorEvent::Dtmf { call_id, .. }
        | OperatorEvent::Rejected { call_id, .. }
        | OperatorEvent::Unavailable { call_id }
        | OperatorEvent::Ended { call_id }
        | OperatorEvent::Cancelled { call_id } => call_id,
    }
}

fn is_terminal_command(command: &OperatorCommand) -> bool {
    matches!(
        command,
        OperatorCommand::CancelCall { .. }
            | OperatorCommand::HangupCall { .. }
            | OperatorCommand::RejectCall { .. }
    )
}

fn is_terminal_event(event: &OperatorEvent) -> bool {
    matches!(
        event,
        OperatorEvent::Rejected { .. }
            | OperatorEvent::Unavailable { .. }
            | OperatorEvent::Ended { .. }
            | OperatorEvent::Cancelled { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        connectivity::core::voice::parse_audio_sdp,
        connectivity::modems::softstack::volte::vilte::parse_video_sdp,
        platform::config::PathLayerConfig,
        services::trunk::bridge::{
            DtmfCapabilities, DtmfSignal, DtmfSource, MediaOffer, VideoOffer,
        },
    };
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn policy(order: &[AccessPathKind]) -> VoicePathPolicy {
        policy_layers(
            &order
                .iter()
                .copied()
                .map(|kind| (kind, true))
                .collect::<Vec<_>>(),
        )
    }

    fn policy_layers(layers: &[(AccessPathKind, bool)]) -> VoicePathPolicy {
        VoicePathPolicy {
            priority: layers
                .iter()
                .map(|(kind, enabled)| PathLayerConfig {
                    kind: *kind,
                    enabled: *enabled,
                })
                .collect(),
            gateway_mode: true,
        }
    }

    fn start(call_id: &str) -> OperatorCommand {
        let sdp = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
        OperatorCommand::StartCall {
            call_id: call_id.to_string(),
            caller: "6108".into(),
            callee: "+601112023012".into(),
            trunk_local_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            offer: MediaOffer {
                audio: parse_audio_sdp(sdp).unwrap(),
                audio_endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, 40000)),
                video: None,
                dtmf: DtmfCapabilities {
                    rtp_event: None,
                    sip_info: true,
                    preferred: DtmfSource::SipInfo,
                },
            },
        }
    }

    async fn wait_available(link: &OperatorLink) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !link.is_available() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    async fn recv_command(
        receiver: &mut tokio::sync::broadcast::Receiver<OperatorCommand>,
    ) -> OperatorCommand {
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("operator command timed out")
            .expect("operator command channel closed")
    }

    #[tokio::test]
    async fn pins_all_commands_to_the_selected_access_leg() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let mut volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi, AccessPathKind::Volte]),
            vec![
                (AccessPathKind::Vowifi, vowifi),
                (AccessPathKind::Volte, volte),
            ],
        );
        let trunk = router.operator_link();
        wait_available(&trunk).await;

        trunk.send_command(start("call-a")).unwrap();
        assert!(matches!(
            recv_command(&mut vowifi_commands).await,
            OperatorCommand::StartCall { .. }
        ));
        assert!(matches!(
            volte_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));

        router.set_policy(policy(&[AccessPathKind::Volte, AccessPathKind::Vowifi]));
        trunk
            .send_command(OperatorCommand::HangupCall {
                call_id: "call-a".into(),
            })
            .unwrap();
        assert!(matches!(
            recv_command(&mut vowifi_commands).await,
            OperatorCommand::HangupCall { .. }
        ));
        assert!(matches!(
            volte_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn unavailable_outgoing_leg_fails_over_without_exposing_failure() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let mut volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi, AccessPathKind::Volte]),
            vec![
                (AccessPathKind::Vowifi, vowifi.clone()),
                (AccessPathKind::Volte, volte),
            ],
        );
        let trunk = router.operator_link();
        let mut trunk_events = trunk.subscribe_events();
        wait_available(&trunk).await;

        trunk.send_command(start("call-b")).unwrap();
        let _ = recv_command(&mut vowifi_commands).await;
        vowifi.send_event(OperatorEvent::Unavailable {
            call_id: "call-b".into(),
        });
        assert!(matches!(
            recv_command(&mut volte_commands).await,
            OperatorCommand::StartCall { call_id, .. } if call_id == "call-b"
        ));
        assert!(matches!(
            trunk_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn video_calls_skip_backends_without_video_capability() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let mut volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi, AccessPathKind::Volte]),
            vec![
                (AccessPathKind::Vowifi, vowifi.clone()),
                (AccessPathKind::Volte, volte.clone()),
            ],
        );
        router.set_backend_video_enabled(AccessPathKind::Volte, true);
        assert!(!vowifi.video_enabled());
        assert!(volte.video_enabled());
        let trunk = router.operator_link();
        wait_available(&trunk).await;

        let mut command = start("video-call");
        let OperatorCommand::StartCall { offer, .. } = &mut command else {
            unreachable!();
        };
        let video_sdp = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=video 50000 RTP/AVP 99\r\na=rtpmap:99 H264/90000\r\na=fmtp:99 packetization-mode=1;profile-level-id=42e01f\r\na=sendrecv\r\n";
        offer.video = Some(VideoOffer {
            description: parse_video_sdp(video_sdp).unwrap(),
            endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, 50000)),
        });

        trunk.send_command(command).unwrap();
        assert!(matches!(
            recv_command(&mut volte_commands).await,
            OperatorCommand::StartCall { call_id, offer, .. }
                if call_id == "video-call" && offer.video.is_some()
        ));
        assert!(matches!(
            vowifi_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn routes_dtmf_bidirectionally_on_the_selected_leg() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let mut volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi, AccessPathKind::Volte]),
            vec![
                (AccessPathKind::Vowifi, vowifi.clone()),
                (AccessPathKind::Volte, volte),
            ],
        );
        let trunk = router.operator_link();
        let mut trunk_events = trunk.subscribe_events();
        wait_available(&trunk).await;

        trunk.send_command(start("dtmf-call")).unwrap();
        let _ = recv_command(&mut vowifi_commands).await;
        vowifi.send_event(OperatorEvent::Answered {
            call_id: "dtmf-call".into(),
            body: Vec::new(),
        });
        let _ = tokio::time::timeout(Duration::from_secs(1), trunk_events.recv())
            .await
            .unwrap()
            .unwrap();

        let outbound = DtmfSignal {
            digit: '5',
            duration_ms: 240,
            source: DtmfSource::SipInfo,
        };
        trunk
            .send_command(OperatorCommand::SendDtmf {
                call_id: "dtmf-call".into(),
                signal: outbound,
            })
            .unwrap();
        assert!(matches!(
            recv_command(&mut vowifi_commands).await,
            OperatorCommand::SendDtmf { call_id, signal }
                if call_id == "dtmf-call" && signal == outbound
        ));
        assert!(matches!(
            volte_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));

        let inbound = DtmfSignal {
            digit: '8',
            duration_ms: 180,
            source: DtmfSource::SipInfo,
        };
        vowifi.send_event(OperatorEvent::Dtmf {
            call_id: "dtmf-call".into(),
            signal: inbound,
        });
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), trunk_events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Dtmf { call_id, signal }
                if call_id == "dtmf-call" && signal == inbound
        ));
    }

    #[tokio::test]
    async fn rejects_incoming_calls_from_a_policy_disabled_leg() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let _volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy_layers(&[
                (AccessPathKind::Volte, true),
                (AccessPathKind::Vowifi, false),
            ]),
            vec![
                (AccessPathKind::Vowifi, vowifi.clone()),
                (AccessPathKind::Volte, volte),
            ],
        );
        let mut trunk_events = router.operator_link().subscribe_events();
        vowifi.send_event(OperatorEvent::Incoming {
            call_id: "ims-call-a".into(),
            caller: "+601112023012".into(),
            body: Vec::new(),
        });

        assert!(matches!(
            recv_command(&mut vowifi_commands).await,
            OperatorCommand::RejectCall { call_id, status: 480 } if call_id == "ims-call-a"
        ));
        assert!(matches!(
            trunk_events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}
