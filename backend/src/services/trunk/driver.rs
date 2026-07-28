//! Per-line Asterisk endpoint driver (D4-D5).
//!
//! `outbound_register` performs REGISTER, standard Digest authentication,
//! refresh and bounded backoff. `static_peer` opens the same bidirectional UDP
//! endpoint without REGISTER. D5 adds the event-driven SIP dialog bridge;
//! until an IMS voice session is attached, new calls receive 100 Trying
//! followed by an honest 480.

use std::{net::SocketAddr, time::Duration};

use chrono::{SecondsFormat, Utc};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::{
    connectivity::core::sip_frame,
    platform::config::{TrunkIpConnectMode, TrunkProfileConfig, TrunkRegistrationMode},
    services::trunk::{
        bridge::{BridgeError, OperatorAvailability, OperatorEvent, TrunkBridge},
        digest,
        operator::OperatorLink,
        runtime::{TrunkPhase, TrunkStage, TrunkStateWriter},
        sip,
        transport::{self, TrunkUdpTransport},
    },
};

const REGISTER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIVE_POLL: Duration = Duration::from_secs(1);
const STATIC_KEEPALIVE: Duration = Duration::from_secs(25);
const MAX_BACKOFF_SECS: u64 = 300;

pub(crate) async fn run(
    profile: TrunkProfileConfig,
    state: TrunkStateWriter,
    mut shutdown: watch::Receiver<bool>,
    operator: OperatorLink,
) {
    let started_at = timestamp_now();
    state
        .update(|snapshot| {
            snapshot.phase = TrunkPhase::Starting;
            snapshot.stage = TrunkStage::Resolving;
            snapshot.started_at = Some(started_at);
            snapshot.last_error = None;
            snapshot.next_retry_at = None;
        })
        .await;

    let mut failure_count = 0u32;
    loop {
        if !state.is_current() || *shutdown.borrow() {
            return;
        }
        match run_session(&profile, &state, &mut shutdown, &operator).await {
            Ok(()) => return,
            Err(error) if state.is_current() => {
                failure_count = failure_count.saturating_add(1);
                let backoff = backoff_duration(failure_count);
                let next_retry_at = timestamp_after(backoff);
                warn!(
                    peer = %format!("{}:{}", profile.asterisk_host, profile.asterisk_port),
                    mode = ?profile.registration_mode,
                    error = %error,
                    retry_secs = backoff.as_secs(),
                    "Asterisk trunk session degraded"
                );
                state
                    .update(|snapshot| {
                        snapshot.phase = TrunkPhase::Degraded;
                        snapshot.stage = TrunkStage::Backoff;
                        snapshot.registered = false;
                        snapshot.last_error = Some(error);
                        snapshot.next_retry_at = Some(next_retry_at);
                        snapshot.reconnect_count = snapshot.reconnect_count.saturating_add(1);
                        snapshot.active_dialogs = 0;
                        snapshot.active_calls = 0;
                    })
                    .await;
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    changed = shutdown.changed() => {
                        if changed.is_ok() && *shutdown.borrow() {
                            return;
                        }
                    }
                }
                state
                    .update(|snapshot| {
                        snapshot.phase = TrunkPhase::Starting;
                        snapshot.stage = TrunkStage::Resolving;
                        snapshot.next_retry_at = None;
                    })
                    .await;
            }
            Err(_) => return,
        }
    }
}

async fn run_session(
    profile: &TrunkProfileConfig,
    state: &TrunkStateWriter,
    shutdown: &mut watch::Receiver<bool>,
    operator: &OperatorLink,
) -> Result<(), String> {
    validate_profile(profile)?;
    state
        .update(|snapshot| snapshot.stage = TrunkStage::Resolving)
        .await;
    let peer_host = if profile.registration_mode == TrunkRegistrationMode::StaticPeer {
        profile
            .match_host
            .as_deref()
            .filter(|host| !host.trim().is_empty())
            .unwrap_or(&profile.asterisk_host)
    } else {
        &profile.asterisk_host
    };
    let addresses = transport::resolve(peer_host, profile.asterisk_port).await?;
    state
        .update(|snapshot| snapshot.stage = TrunkStage::Connecting)
        .await;
    let transport = connect_any(&addresses, profile.local_port).await?;
    let peer = transport.peer_addr().to_string();
    let local_addr = transport.local_addr()?;
    state
        .update(|snapshot| {
            snapshot.peer = Some(peer);
            snapshot.local_endpoint = Some(local_addr.to_string());
        })
        .await;
    let local_aor = format!("sip:{}@{}", profile.username, profile.asterisk_host);
    let mut bridge =
        TrunkBridge::new(local_addr, local_aor).with_operator(OperatorAvailability::Unavailable);
    if !profile.outgoing_binding.trim().is_empty() {
        bridge = bridge.with_outgoing_binding(profile.outgoing_binding.clone());
    }
    if !profile.incoming_binding.trim().is_empty() {
        bridge = bridge.with_asterisk_target(format!(
            "sip:{}@{}:{}",
            profile.incoming_binding, profile.asterisk_host, profile.asterisk_port
        ));
    }
    operator.set_trunk_local_ip(
        (!profile.incoming_binding.trim().is_empty()).then_some(local_addr.ip()),
    );
    operator.set_incoming_mode(profile.incoming_mode);
    operator.set_ip_connect_mode(profile.ip_connect_mode);
    tracing::info!(
        incoming_mode = ?profile.incoming_mode,
        incoming_binding = %profile.incoming_binding,
        outgoing_binding = %profile.outgoing_binding,
        ip_connect_mode = ?profile.ip_connect_mode,
        "Asterisk trunk call routing active"
    );
    let result = match profile.registration_mode {
        TrunkRegistrationMode::StaticPeer => {
            run_static_peer(transport, state, shutdown, &mut bridge, operator).await
        }
        TrunkRegistrationMode::OutboundRegister => {
            run_outbound_register(transport, profile, state, shutdown, &mut bridge, operator).await
        }
    };
    operator.set_trunk_local_ip(None);
    operator.set_incoming_mode(crate::platform::config::TrunkIncomingMode::default());
    operator.set_ip_connect_mode(TrunkIpConnectMode::default());
    result
}

async fn connect_any(
    addresses: &[SocketAddr],
    local_port: u16,
) -> Result<TrunkUdpTransport, String> {
    let mut last_error = None;
    for address in addresses {
        match TrunkUdpTransport::connect(*address, local_port).await {
            Ok(transport) => return Ok(transport),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| "trunk_peer_unreachable".to_string()))
}

async fn run_static_peer(
    transport: TrunkUdpTransport,
    state: &TrunkStateWriter,
    shutdown: &mut watch::Receiver<bool>,
    bridge: &mut TrunkBridge,
    operator: &OperatorLink,
) -> Result<(), String> {
    // A harmless RFC 5626-style CRLF keepalive exposes the selected source port
    // to the configured peer and NAT without pretending to REGISTER.
    transport.send(b"\r\n").await?;
    state
        .update(|snapshot| {
            snapshot.phase = TrunkPhase::Ready;
            snapshot.stage = TrunkStage::Listening;
            snapshot.registered = false;
            snapshot.last_error = None;
            snapshot.next_retry_at = None;
        })
        .await;
    let mut next_keepalive = tokio::time::Instant::now() + STATIC_KEEPALIVE;
    let mut operator_events = operator.subscribe_events();
    loop {
        if !state.is_current() || *shutdown.borrow() {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        if now >= next_keepalive {
            transport.send(b"\r\n").await?;
            next_keepalive = now + STATIC_KEEPALIVE;
        }
        tokio::select! {
            received = transport.recv(RECEIVE_POLL) => match received {
                Ok(frame) => handle_inbound(&transport, &frame, bridge, Some(operator), state).await?,
                Err(error) if error == "trunk_udp_receive_timeout" => {}
                Err(error) => return Err(error),
            },
            event = operator_events.recv() => {
                if let Ok(event) = event {
                    handle_operator_event(&transport, bridge, event, state).await?;
                }
            },
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_outbound_register(
    transport: TrunkUdpTransport,
    profile: &TrunkProfileConfig,
    state: &TrunkStateWriter,
    shutdown: &mut watch::Receiver<bool>,
    bridge: &mut TrunkBridge,
    operator: &OperatorLink,
) -> Result<(), String> {
    let mut dialog = sip::RegisterDialog::fresh();
    let mut operator_events = operator.subscribe_events();
    loop {
        let expiry = register_transaction(&transport, profile, state, &mut dialog, bridge).await?;
        let refresh_after = Duration::from_secs((u64::from(expiry) * 85 / 100).max(30));
        let registered_at = timestamp_now();
        let expires_at = timestamp_after(Duration::from_secs(u64::from(expiry)));
        tracing::info!(
            peer = %transport.peer_addr(),
            local = %transport.local_addr()?,
            expiry_secs = expiry,
            refresh_after_secs = refresh_after.as_secs(),
            "Asterisk trunk registration active"
        );
        state
            .update(|snapshot| {
                snapshot.phase = TrunkPhase::Registered;
                snapshot.stage = TrunkStage::Registered;
                snapshot.registered = true;
                snapshot.registered_at = Some(registered_at);
                snapshot.expires_at = Some(expires_at);
                snapshot.next_retry_at = None;
                snapshot.last_error = None;
            })
            .await;
        let deadline = tokio::time::Instant::now() + refresh_after;
        while tokio::time::Instant::now() < deadline {
            if !state.is_current() {
                return Ok(());
            }
            tokio::select! {
                received = transport.recv(RECEIVE_POLL) => match received {
                    Ok(frame) => handle_inbound(&transport, &frame, bridge, Some(operator), state).await?,
                    Err(error) if error == "trunk_udp_receive_timeout" => {}
                    Err(error) => return Err(error),
                },
                event = operator_events.recv() => {
                    if let Ok(event) = event {
                        handle_operator_event(&transport, bridge, event, state).await?;
                    }
                },
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        state.update(|snapshot| {
                            snapshot.phase = TrunkPhase::Stopping;
                            snapshot.stage = TrunkStage::Stopping;
                        }).await;
                        if let Err(error) = unregister_transaction(
                            &transport,
                            profile,
                            &mut dialog,
                            bridge,
                            state,
                        ).await {
                            warn!(error = %error, "Asterisk trunk unregister failed during shutdown");
                        }
                        return Ok(());
                    }
                }
            }
        }
        state
            .update(|snapshot| {
                snapshot.phase = TrunkPhase::Starting;
                snapshot.stage = TrunkStage::Registering;
                snapshot.registered = false;
            })
            .await;
    }
}

async fn register_transaction(
    transport: &TrunkUdpTransport,
    profile: &TrunkProfileConfig,
    state: &TrunkStateWriter,
    dialog: &mut sip::RegisterDialog,
    bridge: &mut TrunkBridge,
) -> Result<u32, String> {
    let mut authorization = None;
    let mut expires = profile.register_expiry_secs.clamp(60, 86_400);
    for challenge_round in 0..3u32 {
        state
            .update(|snapshot| {
                snapshot.phase = TrunkPhase::Starting;
                snapshot.stage = TrunkStage::Registering;
                snapshot.registered = false;
                snapshot.register_attempts = snapshot.register_attempts.saturating_add(1);
            })
            .await;
        let request = sip::build_register(
            &profile.username,
            &profile.asterisk_host,
            profile.asterisk_port,
            transport.local_addr()?,
            dialog,
            expires,
            authorization.as_deref(),
        )?;
        let expected_cseq = sip_frame::header_value(&request, "CSeq")
            .and_then(|value| value.split_whitespace().next()?.parse::<u32>().ok())
            .ok_or_else(|| "trunk_register_cseq_invalid".to_string())?;
        let response =
            send_register_and_receive(transport, &request, expected_cseq, bridge, state).await?;
        let status = sip::status(&response)?;
        state
            .update(|snapshot| snapshot.last_sip_status = Some(status))
            .await;
        match status {
            200..=299 => return Ok(sip::response_expiry(&response, expires).clamp(60, 86_400)),
            401 | 407 => {
                if profile.secret.is_empty() {
                    return Err("trunk_digest_secret_missing".to_string());
                }
                let proxy = status == 407;
                let challenge_header = if proxy {
                    "Proxy-Authenticate"
                } else {
                    "WWW-Authenticate"
                };
                let value = sip_frame::header_value(&response, challenge_header)
                    .ok_or_else(|| "trunk_digest_challenge_missing".to_string())?;
                let challenge = digest::parse_challenge(&value, proxy)?;
                authorization = Some(digest::build_authorization(
                    &challenge,
                    &profile.username,
                    &profile.secret,
                    "REGISTER",
                    &sip::registrar_uri(&profile.asterisk_host, profile.asterisk_port),
                    &sip::token(12),
                    challenge_round + 1,
                )?);
            }
            423 => {
                expires = sip::min_expires(&response)
                    .filter(|minimum| *minimum <= 86_400)
                    .ok_or_else(|| "trunk_register_min_expires_invalid".to_string())?;
            }
            _ => return Err(format!("trunk_register_rejected:{status}")),
        }
    }
    Err("trunk_register_challenge_limit".to_string())
}

async fn unregister_transaction(
    transport: &TrunkUdpTransport,
    profile: &TrunkProfileConfig,
    dialog: &mut sip::RegisterDialog,
    bridge: &mut TrunkBridge,
    state: &TrunkStateWriter,
) -> Result<(), String> {
    let mut authorization = None;
    for challenge_round in 0..3u32 {
        let request = sip::build_register(
            &profile.username,
            &profile.asterisk_host,
            profile.asterisk_port,
            transport.local_addr()?,
            dialog,
            0,
            authorization.as_deref(),
        )?;
        let expected_cseq = sip_frame::header_value(&request, "CSeq")
            .and_then(|value| value.split_whitespace().next()?.parse::<u32>().ok())
            .ok_or_else(|| "trunk_unregister_cseq_invalid".to_string())?;
        let response =
            send_register_and_receive(transport, &request, expected_cseq, bridge, state).await?;
        let status = sip::status(&response)?;
        match status {
            200..=299 => {
                debug!(peer = %transport.peer_addr(), "Asterisk trunk unregistered");
                return Ok(());
            }
            401 | 407 => {
                if profile.secret.is_empty() {
                    return Err("trunk_digest_secret_missing".to_string());
                }
                let proxy = status == 407;
                let challenge_header = if proxy {
                    "Proxy-Authenticate"
                } else {
                    "WWW-Authenticate"
                };
                let value = sip_frame::header_value(&response, challenge_header)
                    .ok_or_else(|| "trunk_digest_challenge_missing".to_string())?;
                let challenge = digest::parse_challenge(&value, proxy)?;
                authorization = Some(digest::build_authorization(
                    &challenge,
                    &profile.username,
                    &profile.secret,
                    "REGISTER",
                    &sip::registrar_uri(&profile.asterisk_host, profile.asterisk_port),
                    &sip::token(12),
                    challenge_round + 1,
                )?);
            }
            _ => return Err(format!("trunk_unregister_rejected:{status}")),
        }
    }
    Err("trunk_unregister_challenge_limit".to_string())
}

async fn send_register_and_receive(
    transport: &TrunkUdpTransport,
    request: &[u8],
    expected_cseq: u32,
    bridge: &mut TrunkBridge,
    state: &TrunkStateWriter,
) -> Result<Vec<u8>, String> {
    let deadline = tokio::time::Instant::now() + REGISTER_RESPONSE_TIMEOUT;
    let mut retransmit_after = Duration::from_millis(500);
    transport.send(request).await?;
    record_sip_tx(state, request).await;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err("trunk_register_timeout".to_string());
        }
        let wait = (deadline - now).min(retransmit_after);
        match transport.recv(wait).await {
            Ok(frame) if sip::is_request(&frame) => {
                handle_inbound(transport, &frame, bridge, None, state).await?
            }
            Ok(frame) => {
                record_sip_rx(state, &frame).await;
                let cseq = sip_frame::header_value(&frame, "CSeq").unwrap_or_default();
                let mut parts = cseq.split_whitespace();
                let matching = parts.next().and_then(|value| value.parse::<u32>().ok())
                    == Some(expected_cseq)
                    && parts
                        .next()
                        .is_some_and(|method| method.eq_ignore_ascii_case("REGISTER"));
                if matching {
                    let status = sip::status(&frame)?;
                    if status >= 200 {
                        return Ok(frame);
                    }
                }
            }
            Err(error) if error == "trunk_udp_receive_timeout" => {
                transport.send(request).await?;
                record_sip_tx(state, request).await;
                retransmit_after = (retransmit_after * 2).min(Duration::from_secs(2));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn handle_inbound(
    transport: &TrunkUdpTransport,
    frame: &[u8],
    bridge: &mut TrunkBridge,
    operator: Option<&OperatorLink>,
    state: &TrunkStateWriter,
) -> Result<(), String> {
    if frame == b"\r\n" || frame == b"\r\n\r\n" {
        return Ok(());
    }
    record_sip_rx(state, frame).await;
    if sip_frame::is_request(frame, "INVITE") {
        let is_reinvite =
            crate::services::trunk::dialog::call_id(frame).is_some_and(|call_id| bridge.has_call(&call_id));
        state
            .update(|snapshot| {
                if is_reinvite {
                    snapshot.reinvite_count = snapshot.reinvite_count.saturating_add(1);
                } else {
                    snapshot.invite_count = snapshot.invite_count.saturating_add(1);
                }
            })
            .await;
    }
    if sip_frame::is_request(frame, "REGISTER") {
        let response = sip::build_response(frame, 405, "Method Not Allowed")?;
        transport.send(&response).await?;
        record_sip_tx(state, &response).await;
        return Ok(());
    }
    let operator_available = operator.is_some_and(OperatorLink::is_available);
    bridge.set_operator(if operator_available {
        OperatorAvailability::EventDriven
    } else {
        OperatorAvailability::Unavailable
    });
    let output = match bridge.handle_asterisk(frame) {
        Ok(output) => output,
        Err(error) if sip::is_request(frame) => {
            let (status, reason) = match &error {
                BridgeError::UnsupportedMedia(_) => (488, "Not Acceptable Here"),
                BridgeError::InvalidState(_) => (481, "Call/Transaction Does Not Exist"),
                BridgeError::MalformedRequest(_) => (400, "Bad Request"),
                BridgeError::Forbidden(_) => (403, "Forbidden"),
            };
            warn!(status, error = %error, "Rejecting invalid Asterisk trunk request");
            if let Ok(response) = sip::build_response(frame, status, reason) {
                transport.send(&response).await?;
                record_sip_tx(state, &response).await;
            }
            return Ok(());
        }
        Err(error) => {
            debug!(error = %error, "Ignoring unrelated Asterisk trunk response");
            return Ok(());
        }
    };
    for response in output.asterisk_frames {
        transport.send(&response).await?;
        record_sip_tx(state, &response).await;
    }
    if !operator_available {
        return Ok(());
    }
    for command in output.operator_commands {
        let result = operator
            .expect("available operator link")
            .send_command(command);
        if let Err(command) = result {
            let call_id = match *command {
                crate::services::trunk::bridge::OperatorCommand::StartCall { call_id, .. } => Some(call_id),
                _ => None,
            };
            if let Some(call_id) = call_id {
                let unavailable = bridge
                    .handle_operator_event(crate::services::trunk::bridge::OperatorEvent::Unavailable {
                        call_id,
                    })
                    .map_err(|error| error.to_string())?;
                for response in unavailable.asterisk_frames {
                    transport.send(&response).await?;
                    record_sip_tx(state, &response).await;
                }
            }
        }
    }
    sync_bridge_diagnostics(state, bridge).await;
    Ok(())
}

async fn handle_operator_event(
    transport: &TrunkUdpTransport,
    bridge: &mut TrunkBridge,
    event: OperatorEvent,
    state: &TrunkStateWriter,
) -> Result<(), String> {
    let output = match bridge.handle_operator_event(event) {
        Ok(output) => output,
        Err(BridgeError::InvalidState(error)) => {
            debug!(error, "Ignoring stale operator event");
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    for frame in output.asterisk_frames {
        transport.send(&frame).await?;
        record_sip_tx(state, &frame).await;
    }
    sync_bridge_diagnostics(state, bridge).await;
    Ok(())
}

async fn record_sip_rx(state: &TrunkStateWriter, frame: &[u8]) {
    let is_invite = sip_frame::is_request(frame, "INVITE");
    let has_video =
        is_invite && crate::connectivity::modems::softstack::volte::vilte::parse_video_sdp(sip_frame::body(frame)).is_ok();
    state
        .update(|snapshot| {
            snapshot.sip_rx_frames = snapshot.sip_rx_frames.saturating_add(1);
            snapshot.sip_rx_bytes = snapshot.sip_rx_bytes.saturating_add(frame.len() as u64);
            snapshot.last_activity_at = Some(timestamp_now());
            if is_invite {
                snapshot.media_negotiations = snapshot.media_negotiations.saturating_add(1);
            }
            if has_video {
                snapshot.video_negotiations = snapshot.video_negotiations.saturating_add(1);
            }
        })
        .await;
}

async fn record_sip_tx(state: &TrunkStateWriter, frame: &[u8]) {
    state
        .update(|snapshot| {
            snapshot.sip_tx_frames = snapshot.sip_tx_frames.saturating_add(1);
            snapshot.sip_tx_bytes = snapshot.sip_tx_bytes.saturating_add(frame.len() as u64);
            snapshot.last_activity_at = Some(timestamp_now());
        })
        .await;
}

async fn sync_bridge_diagnostics(state: &TrunkStateWriter, bridge: &TrunkBridge) {
    state
        .update(|snapshot| {
            snapshot.active_dialogs = bridge.active_call_count() as u64;
            snapshot.active_calls = bridge.confirmed_call_count() as u64;
        })
        .await;
}

fn validate_profile(profile: &TrunkProfileConfig) -> Result<(), String> {
    if profile.asterisk_host.trim().is_empty() {
        return Err("trunk_asterisk_host_required".to_string());
    }
    if profile.asterisk_port == 0 {
        return Err("trunk_asterisk_port_invalid".to_string());
    }
    if profile.local_port == 0 {
        return Err("trunk_local_port_required".to_string());
    }
    if profile.registration_mode == TrunkRegistrationMode::OutboundRegister
        && profile.username.trim().is_empty()
    {
        return Err("trunk_username_required".to_string());
    }
    if profile.registration_mode == TrunkRegistrationMode::OutboundRegister
        && !(60..=86_400).contains(&profile.register_expiry_secs)
    {
        return Err("trunk_register_expiry_invalid".to_string());
    }
    Ok(())
}

fn backoff_duration(failure_count: u32) -> Duration {
    let exponent = failure_count.saturating_sub(1).min(6);
    Duration::from_secs((5u64.saturating_mul(1u64 << exponent)).min(MAX_BACKOFF_SECS))
}

fn timestamp_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn timestamp_after(duration: Duration) -> String {
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    (Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::trunk::{
        bridge::{OperatorCommand, OperatorEvent},
        operator::OperatorLink,
        runtime::TrunkRuntime,
    };
    use std::net::Ipv4Addr;
    use tokio::net::UdpSocket;

    async fn wait_for_phase(runtime: &TrunkRuntime, phase: &str) {
        for _ in 0..100 {
            if runtime.status().await.phase == phase {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "runtime did not reach {phase}: {:?}",
            runtime.status().await
        );
    }

    async fn free_udp_port() -> u16 {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        socket.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn outbound_register_completes_digest_challenge() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let mock = tokio::spawn(async move {
            let mut frame = [0u8; 8192];
            let (read, peer) = server.recv_from(&mut frame).await.unwrap();
            let first = String::from_utf8_lossy(&frame[..read]);
            assert!(first.starts_with("REGISTER sip:127.0.0.1:"));
            assert!(!first.contains("Authorization:"));
            let cseq = sip_frame::header_value(&frame[..read], "CSeq").unwrap();
            server
                .send_to(
                    format!(
                        "SIP/2.0 401 Unauthorized\r\nCSeq: {cseq}\r\nWWW-Authenticate: Digest realm=\"pbx\", nonce=\"abc\", algorithm=MD5, qop=\"auth\"\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                    peer,
                )
                .await
                .unwrap();
            let (read, peer) = server.recv_from(&mut frame).await.unwrap();
            let authenticated = String::from_utf8_lossy(&frame[..read]);
            assert!(authenticated.contains("Authorization: Digest username=\"4101\""));
            assert!(authenticated.contains("algorithm=MD5"));
            let cseq = sip_frame::header_value(&frame[..read], "CSeq").unwrap();
            server
                .send_to(
                    format!(
                        "SIP/2.0 200 OK\r\nCSeq: {cseq}\r\nExpires: 60\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                    peer,
                )
                .await
                .unwrap();
            let (read, peer) = server.recv_from(&mut frame).await.unwrap();
            let unregister = String::from_utf8_lossy(&frame[..read]);
            assert!(unregister.contains("Expires: 0\r\n"));
            assert!(unregister.contains(";expires=0"));
            let cseq = sip_frame::header_value(&frame[..read], "CSeq").unwrap();
            server
                .send_to(
                    format!(
                        "SIP/2.0 200 OK\r\nCSeq: {cseq}\r\nExpires: 0\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                    peer,
                )
                .await
                .unwrap();
        });

        let runtime = TrunkRuntime::new();
        let local_port = free_udp_port().await;
        runtime
            .activate_profile(&TrunkProfileConfig {
                enabled: true,
                registration_mode: TrunkRegistrationMode::OutboundRegister,
                asterisk_host: server_addr.ip().to_string(),
                asterisk_port: server_addr.port(),
                local_port,
                username: "4101".to_string(),
                secret: "secret".to_string(),
                register_expiry_secs: 60,
                ..TrunkProfileConfig::default()
            })
            .await;
        wait_for_phase(&runtime, "registered").await;
        let status = runtime.status().await;
        assert!(status.registered);
        assert_eq!(status.last_sip_status, Some(200));
        assert_eq!(status.register_attempts, 2);
        runtime
            .activate_profile(&TrunkProfileConfig::default())
            .await;
        mock.await.unwrap();
    }

    #[tokio::test]
    async fn static_peer_listens_and_answers_options() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let local_port = free_udp_port().await;
        let runtime = TrunkRuntime::new();
        runtime
            .activate_profile(&TrunkProfileConfig {
                enabled: true,
                registration_mode: TrunkRegistrationMode::StaticPeer,
                asterisk_host: server_addr.ip().to_string(),
                asterisk_port: server_addr.port(),
                local_port,
                ..TrunkProfileConfig::default()
            })
            .await;
        let mut frame = [0u8; 8192];
        let (read, peer) =
            tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut frame))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&frame[..read], b"\r\n");
        let request = b"OPTIONS sip:4101@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bK1\r\nFrom: <sip:pbx@local>;tag=a\r\nTo: <sip:4101@simadmin>\r\nCall-ID: options-1\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n";
        server.send_to(request, peer).await.unwrap();
        let (read, _) = tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut frame))
            .await
            .unwrap()
            .unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 200 OK"));
        wait_for_phase(&runtime, "ready").await;
        let status = runtime.status().await;
        assert_eq!(status.stage, "listening");
        assert!(!status.registered);

        let sdp = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n";
        let invite = format!(
            "INVITE sip:4101@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKcall\r\nFrom: <sip:6108@pbx>;tag=pbx-a\r\nTo: <sip:4101@simadmin>\r\nCall-ID: static-call-1\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            server_addr.port(),
            sdp.len(),
            String::from_utf8_lossy(sdp)
        );
        server.send_to(invite.as_bytes(), peer).await.unwrap();
        let (read, _) = tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut frame))
            .await
            .unwrap()
            .unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 100 Trying"));
        let (read, _) = tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut frame))
            .await
            .unwrap()
            .unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 480 Temporarily Unavailable"));
        assert_eq!(runtime.status().await.phase, "ready");
        runtime
            .activate_profile(&TrunkProfileConfig::default())
            .await;
    }

    #[tokio::test]
    async fn event_link_drives_asterisk_call_dialog_without_blocking_udp() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let local_port = free_udp_port().await;
        let operator = OperatorLink::default();
        let mut commands = operator.subscribe_commands();
        operator.set_ready(true);
        let runtime = TrunkRuntime::with_operator(operator.clone());
        runtime
            .activate_profile(&TrunkProfileConfig {
                enabled: true,
                registration_mode: TrunkRegistrationMode::StaticPeer,
                asterisk_host: server_addr.ip().to_string(),
                asterisk_port: server_addr.port(),
                local_port,
                ..TrunkProfileConfig::default()
            })
            .await;
        let mut frame = [0u8; 8192];
        let (_, peer) = server.recv_from(&mut frame).await.unwrap();
        let sdp = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\na=sendrecv\r\n";
        let invite = format!(
            "INVITE sip:+8613800138000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKcall\r\nFrom: <sip:6108@pbx>;tag=pbx-a\r\nTo: <sip:41000@simadmin>\r\nCall-ID: linked-call-1\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            server_addr.port(),
            sdp.len(),
            String::from_utf8_lossy(sdp)
        );
        server.send_to(invite.as_bytes(), peer).await.unwrap();
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 100 Trying"));
        let command = commands.recv().await.unwrap();
        let OperatorCommand::StartCall {
            call_id,
            callee,
            trunk_local_ip,
            ..
        } = command
        else {
            panic!("expected StartCall");
        };
        assert_eq!(call_id, "linked-call-1");
        assert_eq!(callee, "sip:+8613800138000@simadmin");
        assert_eq!(trunk_local_ip, std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));

        operator.send_event(OperatorEvent::Provisional {
            call_id: call_id.clone(),
            status: 180,
            body: None,
        });
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 180 Ringing"));
        operator.send_event(OperatorEvent::Answered {
            call_id: call_id.clone(),
            body: sdp.to_vec(),
        });
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        let answer = String::from_utf8_lossy(&frame[..read]);
        assert!(answer.starts_with("SIP/2.0 200 OK"));
        let to = sip_frame::header_value(&frame[..read], "To").unwrap();
        let tag = to.split(";tag=").nth(1).unwrap();
        let ack = format!(
            "ACK sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKack\r\nFrom: <sip:6108@pbx>;tag=pbx-a\r\nTo: <sip:41000@simadmin>;tag={}\r\nCall-ID: linked-call-1\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
            server_addr.port(), tag
        );
        server.send_to(ack.as_bytes(), peer).await.unwrap();
        let bye = format!(
            "BYE sip:41000@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKbye\r\nFrom: <sip:6108@pbx>;tag=pbx-a\r\nTo: <sip:41000@simadmin>;tag={}\r\nCall-ID: linked-call-1\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n",
            server_addr.port(), tag
        );
        server.send_to(bye.as_bytes(), peer).await.unwrap();
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        assert!(frame[..read].starts_with(b"SIP/2.0 200 OK"));
        assert!(matches!(
            commands.recv().await.unwrap(),
            OperatorCommand::HangupCall { call_id } if call_id == "linked-call-1"
        ));
        runtime
            .activate_profile(&TrunkProfileConfig::default())
            .await;
    }

    #[tokio::test]
    async fn operator_incoming_event_dials_extension_and_returns_answer_command() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let operator = OperatorLink::default();
        let mut commands = operator.subscribe_commands();
        operator.set_ready(true);
        let runtime = TrunkRuntime::with_operator(operator.clone());
        runtime
            .activate_profile(&TrunkProfileConfig {
                enabled: true,
                registration_mode: TrunkRegistrationMode::StaticPeer,
                asterisk_host: server_addr.ip().to_string(),
                asterisk_port: server_addr.port(),
                local_port: free_udp_port().await,
                username: "41000".into(),
                incoming_binding: "6108".into(),
                ..TrunkProfileConfig::default()
            })
            .await;
        let mut frame = [0u8; 8192];
        let (_, peer) = server.recv_from(&mut frame).await.unwrap();
        assert_eq!(operator.trunk_local_ip(), Some(Ipv4Addr::LOCALHOST.into()));
        let sdp = b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=call\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n";
        operator.send_event(OperatorEvent::Incoming {
            call_id: "ims-mt-1".into(),
            caller: "sip:+8613800138000@ims.example".into(),
            body: sdp.to_vec(),
        });
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        let invite = frame[..read].to_vec();
        assert!(invite.starts_with(b"INVITE sip:6108@127.0.0.1:"));
        assert_eq!(sip_frame::body(&invite), sdp);
        let via = sip_frame::header_value(&invite, "Via").unwrap();
        let from = sip_frame::header_value(&invite, "From").unwrap();
        let to = sip_frame::header_value(&invite, "To").unwrap();
        let call_id = sip_frame::header_value(&invite, "Call-ID").unwrap();
        let ringing = format!(
            "SIP/2.0 180 Ringing\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=pbx-mt\r\nCall-ID: {call_id}\r\nCSeq: 1 INVITE\r\nContent-Length: 0\r\n\r\n"
        );
        server.send_to(ringing.as_bytes(), peer).await.unwrap();
        assert!(matches!(
            commands.recv().await.unwrap(),
            OperatorCommand::ReportProvisional { call_id, status: 180, body: None }
                if call_id == "ims-mt-1"
        ));
        let answered = format!(
            "SIP/2.0 200 OK\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=pbx-mt\r\nCall-ID: {call_id}\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            sdp.len(),
            String::from_utf8_lossy(sdp),
        );
        server.send_to(answered.as_bytes(), peer).await.unwrap();
        let (read, _) = server.recv_from(&mut frame).await.unwrap();
        assert!(frame[..read].starts_with(b"ACK sip:6108@127.0.0.1:"));
        assert!(matches!(
            commands.recv().await.unwrap(),
            OperatorCommand::AcceptCall { call_id, body }
                if call_id == "ims-mt-1" && body == sdp
        ));
        runtime
            .activate_profile(&TrunkProfileConfig::default())
            .await;
        assert_eq!(operator.trunk_local_ip(), None);
    }

    #[test]
    fn backoff_is_bounded() {
        assert_eq!(backoff_duration(1), Duration::from_secs(5));
        assert_eq!(backoff_duration(3), Duration::from_secs(20));
        assert!(backoff_duration(100) <= Duration::from_secs(MAX_BACKOFF_SECS));
    }
}
