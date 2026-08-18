//! Policy-aware per-line routing between one Asterisk trunk and IMS access legs.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

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
    /// Present only for a local/API call whose media offer varies by access.
    /// A pre-answer failover must rebuild `StartCall` for the next leg instead
    /// of replaying the first leg's codec/payload policy.
    start_plan: Option<VoiceCallPlan>,
}

/// A mobile-originated call prepared for each IMS access that can carry it.
///
/// The router owns access selection, so an HTTP/API caller must not select a
/// VoWiFi link itself merely to send `StartCall`.  Codec policy can differ
/// between the LTE and ePDG catalog records, however, and the selected leg
/// must receive the matching media offer.  This plan keeps both facts
/// together: selection remains centralized while media remains access-aware.
#[derive(Debug, Clone)]
pub struct VoiceCallPlan {
    pub call_id: String,
    pub caller: String,
    pub callee: String,
    pub trunk_local_ip: std::net::IpAddr,
    offers: Vec<(AccessPathKind, super::bridge::MediaOffer)>,
}

impl VoiceCallPlan {
    pub fn new(
        call_id: impl Into<String>,
        caller: impl Into<String>,
        callee: impl Into<String>,
        trunk_local_ip: std::net::IpAddr,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            caller: caller.into(),
            callee: callee.into(),
            trunk_local_ip,
            offers: Vec::new(),
        }
    }

    pub fn with_offer(mut self, access: AccessPathKind, offer: super::bridge::MediaOffer) -> Self {
        if let Some((_, current)) = self
            .offers
            .iter_mut()
            .find(|(candidate, _)| *candidate == access)
        {
            *current = offer;
        } else {
            self.offers.push((access, offer));
        }
        self
    }

    fn command_for(&self, access: AccessPathKind) -> Option<OperatorCommand> {
        self.offers
            .iter()
            .find(|(candidate, _)| *candidate == access)
            .map(|(_, offer)| offer)
            .cloned()
            .map(|offer| OperatorCommand::StartCall {
                call_id: self.call_id.clone(),
                caller: self.caller.clone(),
                callee: self.callee.clone(),
                trunk_local_ip: self.trunk_local_ip,
                offer,
            })
    }
}

/// The initial outcome of routing a locally-originated call. A later
/// `Unavailable` event can still make the router fail over before the dialog
/// is answered; all later lifecycle changes continue on `OperatorLink`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedVoiceCall {
    pub call_id: String,
    pub access: AccessPathKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCallStartError {
    RouterUnavailable,
    RouteTimedOut,
    NoEligibleImsAccess,
}

impl VoiceCallStartError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::RouterUnavailable => "voice_access_router_unavailable",
            Self::RouteTimedOut => "voice_access_router_timeout",
            Self::NoEligibleImsAccess => "voice_ims_access_unavailable",
        }
    }
}

impl std::fmt::Display for VoiceCallStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for VoiceCallStartError {}

enum RouterRequest {
    StartCall {
        plan: VoiceCallPlan,
        response: oneshot::Sender<Result<RoutedVoiceCall, VoiceCallStartError>>,
    },
    CallAccess {
        call_id: String,
        response: oneshot::Sender<Option<AccessPathKind>>,
    },
}

/// Owns the public trunk-facing link and keeps each call pinned to exactly one
/// backend. This prevents two IMS stacks from consuming the same broadcast
/// command when VoLTE and VoWiFi are registered at the same time.
pub struct VoiceAccessRouter {
    trunk: OperatorLink,
    policy: Arc<RwLock<VoicePathPolicy>>,
    backends: Vec<AccessBackend>,
    requests: Option<mpsc::Sender<RouterRequest>>,
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

        let (requests, request_rx) = mpsc::channel(16);

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
                    request_rx,
                )
                .await;
            })
        });

        Self {
            trunk,
            policy,
            backends,
            requests: task.as_ref().map(|_| requests),
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

    /// Select the same preferred registered access used for a new audio call.
    /// Supplementary services use this only to choose a transport; their state
    /// remains network-authoritative and is not duplicated per access.
    pub fn preferred_ready_ims_access(&self) -> Option<AccessPathKind> {
        route_plan(&current_policy(&self.policy), &self.backends, false)
            .into_iter()
            .find(|kind| kind.is_ims())
    }

    /// Route a locally-originated call through the same policy and route table
    /// used by the Asterisk trunk. The selected access is returned only after
    /// its `StartCall` command has been accepted by the registered live leg.
    pub async fn start_call(
        &self,
        plan: VoiceCallPlan,
    ) -> Result<RoutedVoiceCall, VoiceCallStartError> {
        if plan.call_id.trim().is_empty() || plan.callee.trim().is_empty() {
            return Err(VoiceCallStartError::NoEligibleImsAccess);
        }
        let Some(requests) = self.requests.as_ref() else {
            return Err(VoiceCallStartError::RouterUnavailable);
        };
        let (response_tx, response_rx) = oneshot::channel();
        requests
            .send(RouterRequest::StartCall {
                plan,
                response: response_tx,
            })
            .await
            .map_err(|_| VoiceCallStartError::RouterUnavailable)?;
        match tokio::time::timeout(Duration::from_secs(1), response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(VoiceCallStartError::RouterUnavailable),
            Err(_) => Err(VoiceCallStartError::RouteTimedOut),
        }
    }

    /// Return the IMS access that owns an active call. Incoming-call answer
    /// generation uses this to apply the matching carrier codec profile rather
    /// than guessing from whichever registrations happen to be live.
    pub async fn call_access(&self, call_id: &str) -> Option<AccessPathKind> {
        let requests = self.requests.as_ref()?;
        let (response_tx, response_rx) = oneshot::channel();
        requests
            .send(RouterRequest::CallAccess {
                call_id: call_id.to_string(),
                response: response_tx,
            })
            .await
            .ok()?;
        tokio::time::timeout(Duration::from_secs(1), response_rx)
            .await
            .ok()?
            .ok()?
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
    mut requests: mpsc::Receiver<RouterRequest>,
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
            request = requests.recv() => match request {
                Some(RouterRequest::StartCall { plan, response }) => {
                    // A timed-out HTTP caller drops its receiver. Do not start
                    // a dial after reporting an error to that caller.
                    if response.is_closed() {
                        continue;
                    }
                    let result = route_call_plan(plan, &policy, &backends, &mut routes);
                    let _ = response.send(result);
                }
                Some(RouterRequest::CallAccess { call_id, response }) => {
                    let access = routes.get(&call_id).map(|route| route.owner);
                    let _ = response.send(access);
                }
                None => break,
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

fn route_call_plan(
    plan: VoiceCallPlan,
    policy: &RwLock<VoicePathPolicy>,
    backends: &[AccessBackend],
    routes: &mut HashMap<String, CallRoute>,
) -> Result<RoutedVoiceCall, VoiceCallStartError> {
    let candidates = route_plan(&current_policy(policy), backends, false);
    let mut remaining = candidates.clone();
    while let Some(kind) = remaining.first().copied() {
        remaining.remove(0);
        let Some(command) = plan.command_for(kind) else {
            continue;
        };
        let Some(selected) = backend(backends, kind) else {
            continue;
        };
        if selected.link.send_command(command.clone()).is_ok() {
            routes.insert(
                plan.call_id.clone(),
                CallRoute {
                    owner: kind,
                    remaining,
                    start: Some(command),
                    start_plan: Some(plan.clone()),
                },
            );
            return Ok(RoutedVoiceCall {
                call_id: plan.call_id,
                access: kind,
            });
        }
    }
    Err(VoiceCallStartError::NoEligibleImsAccess)
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
            let available =
                backend.link.is_available() && (!video_required || backend.link.video_enabled());
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

    // Keep IMS MT signaling reachable even when no external Asterisk trunk is
    // enabled. The HTTP call API answers against a loopback RTP sink; an
    // active trunk replaces this address with its real local endpoint.
    let local_ip = trunk
        .trunk_local_ip()
        .or(Some(std::net::Ipv4Addr::LOCALHOST.into()));
    let incoming_mode = trunk.incoming_mode();
    let ip_connect_mode = trunk.ip_connect_mode();
    trunk.set_video_enabled(
        candidates.iter().any(|kind| {
            backend(backends, *kind).is_some_and(|backend| backend.link.video_enabled())
        }),
    );
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
                        start_plan: None,
                    },
                );
                return;
            }
        }
        trunk.send_event(OperatorEvent::Unavailable { call_id });
        return;
    }

    let Some(owner) = routes.get(&call_id).map(|route| route.owner) else {
        if matches!(&command, OperatorCommand::TransferCall { .. }) {
            trunk.send_event(OperatorEvent::TransferResponse {
                call_id,
                status: 481,
            });
        } else {
            trunk.send_event(OperatorEvent::Unavailable { call_id });
        }
        return;
    };
    let sent = backend(backends, owner)
        .is_some_and(|selected| selected.link.send_command(command.clone()).is_ok());
    if !sent {
        if matches!(&command, OperatorCommand::TransferCall { .. }) {
            trunk.send_event(OperatorEvent::TransferResponse {
                call_id: call_id.clone(),
                status: 503,
            });
        } else {
            trunk.send_event(OperatorEvent::Unavailable {
                call_id: call_id.clone(),
            });
        }
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
                start_plan: None,
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
        while let Some(next) = route.remaining.first().copied() {
            route.remaining.remove(0);
            let start = route
                .start_plan
                .as_ref()
                .and_then(|plan| plan.command_for(next))
                .or_else(|| route.start.clone());
            let Some(start) = start else {
                continue;
            };
            let Some(selected) = backend(backends, next) else {
                continue;
            };
            if selected.link.send_command(start.clone()).is_ok() {
                route.owner = next;
                route.start = Some(start);
                tracing::warn!(call_id = %call_id, access = next.as_str(), "Voice call failed over to next access leg");
                return;
            }
        }
    }

    if matches!(&event, OperatorEvent::Answered { .. }) {
        route.start = None;
        route.start_plan = None;
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
        | OperatorCommand::SendDtmf { call_id, .. }
        | OperatorCommand::TransferCall { call_id, .. } => call_id,
    }
}

fn event_call_id(event: &OperatorEvent) -> &str {
    match event {
        OperatorEvent::Incoming { call_id, .. }
        | OperatorEvent::Provisional { call_id, .. }
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
        connectivity::core::ims_video::parse_video_sdp,
        connectivity::core::voice::parse_audio_sdp,
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

    fn call_plan(call_id: &str) -> VoiceCallPlan {
        let OperatorCommand::StartCall {
            call_id,
            caller,
            callee,
            trunk_local_ip,
            offer,
        } = start(call_id)
        else {
            unreachable!();
        };
        VoiceCallPlan::new(call_id, caller, callee, trunk_local_ip)
            .with_offer(AccessPathKind::Vowifi, offer.clone())
            .with_offer(AccessPathKind::Volte, offer)
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
    async fn local_call_plan_uses_router_selection_and_returns_queued_access() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        let mut volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Volte, AccessPathKind::Vowifi]),
            vec![
                (AccessPathKind::Vowifi, vowifi),
                (AccessPathKind::Volte, volte),
            ],
        );

        let queued = router
            .start_call(call_plan("local-voicemail-a"))
            .await
            .unwrap();
        assert_eq!(queued.call_id, "local-voicemail-a");
        assert_eq!(queued.access, AccessPathKind::Volte);
        assert!(matches!(
            recv_command(&mut volte_commands).await,
            OperatorCommand::StartCall { call_id, .. } if call_id == "local-voicemail-a"
        ));
        assert!(matches!(
            vowifi_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn local_call_plan_skips_accesses_without_matching_media_offer() {
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
        let OperatorCommand::StartCall {
            call_id,
            caller,
            callee,
            trunk_local_ip,
            offer,
        } = start("local-voicemail-b")
        else {
            unreachable!();
        };
        let plan = VoiceCallPlan::new(call_id, caller, callee, trunk_local_ip)
            .with_offer(AccessPathKind::Volte, offer);

        let queued = router.start_call(plan).await.unwrap();
        assert_eq!(queued.access, AccessPathKind::Volte);
        assert!(matches!(
            recv_command(&mut volte_commands).await,
            OperatorCommand::StartCall { call_id, .. } if call_id == "local-voicemail-b"
        ));
        assert!(matches!(
            vowifi_commands.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn local_call_plan_failover_uses_the_next_access_media_offer() {
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
        let OperatorCommand::StartCall {
            call_id,
            caller,
            callee,
            trunk_local_ip,
            offer,
        } = start("local-voicemail-failover")
        else {
            unreachable!();
        };
        let mut volte_offer = offer.clone();
        volte_offer.audio.media_port = 40002;
        volte_offer.audio_endpoint = SocketAddr::from((Ipv4Addr::LOCALHOST, 40002));
        let plan = VoiceCallPlan::new(call_id, caller, callee, trunk_local_ip)
            .with_offer(AccessPathKind::Vowifi, offer)
            .with_offer(AccessPathKind::Volte, volte_offer);

        let queued = router.start_call(plan).await.unwrap();
        assert_eq!(queued.access, AccessPathKind::Vowifi);
        assert!(matches!(
            recv_command(&mut vowifi_commands).await,
            OperatorCommand::StartCall { offer, .. } if offer.audio.media_port == 40000
        ));

        vowifi.send_event(OperatorEvent::Unavailable {
            call_id: "local-voicemail-failover".into(),
        });
        assert!(matches!(
            recv_command(&mut volte_commands).await,
            OperatorCommand::StartCall { offer, .. } if offer.audio.media_port == 40002
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
    async fn transfer_dispatch_failure_ends_only_the_refer_transaction() {
        let vowifi = OperatorLink::default();
        let mut vowifi_commands = vowifi.subscribe_commands();
        vowifi.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi]),
            vec![(AccessPathKind::Vowifi, vowifi.clone())],
        );
        let trunk = router.operator_link();
        let mut trunk_events = trunk.subscribe_events();
        wait_available(&trunk).await;

        trunk.send_command(start("transfer-call")).unwrap();
        let _ = recv_command(&mut vowifi_commands).await;
        vowifi.send_event(OperatorEvent::Answered {
            call_id: "transfer-call".into(),
            body: Vec::new(),
        });
        let _ = tokio::time::timeout(Duration::from_secs(1), trunk_events.recv())
            .await
            .unwrap()
            .unwrap();
        drop(vowifi_commands);

        trunk
            .send_command(OperatorCommand::TransferCall {
                call_id: "transfer-call".into(),
                refer_to: "sip:+601199999999@ims.example".into(),
            })
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), trunk_events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::TransferResponse { call_id, status: 503 }
                if call_id == "transfer-call"
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

    #[tokio::test]
    async fn reports_the_exact_access_that_owns_an_incoming_call() {
        let vowifi = OperatorLink::default();
        let volte = OperatorLink::default();
        let _vowifi_commands = vowifi.subscribe_commands();
        let _volte_commands = volte.subscribe_commands();
        vowifi.set_ready(true);
        volte.set_ready(true);
        let router = VoiceAccessRouter::new(
            policy(&[AccessPathKind::Vowifi, AccessPathKind::Volte]),
            vec![
                (AccessPathKind::Vowifi, vowifi),
                (AccessPathKind::Volte, volte.clone()),
            ],
        );
        let trunk = router.operator_link();
        wait_available(&trunk).await;
        let mut events = trunk.subscribe_events();
        volte.send_event(OperatorEvent::Incoming {
            call_id: "ims-incoming-access".into(),
            caller: "+601112023012".into(),
            body: Vec::new(),
        });
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Incoming { call_id, .. } if call_id == "ims-incoming-access"
        ));
        assert_eq!(
            router.call_access("ims-incoming-access").await,
            Some(AccessPathKind::Volte)
        );

        volte.send_event(OperatorEvent::Ended {
            call_id: "ims-incoming-access".into(),
        });
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap(),
            OperatorEvent::Ended { .. }
        ));
        assert_eq!(router.call_access("ims-incoming-access").await, None);
    }
}
