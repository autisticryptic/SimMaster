//! Registration-event integration on the retained cellular SIP owner.
//! Protocol parsing, dialog identity and version rules are shared with WiFi.
use super::*;
use crate::connectivity::core::reg_event::{
    classify_unsubscribed_notify, RegEventAction, RegEventBinding, RegEventEffect,
    RegEventFrame, RegEventSubscription,
};

#[derive(Default)]
pub(super) struct Observer {
    binding: Option<RegEventBinding>,
    subscription: Option<RegEventSubscription>,
    pub(super) revoked: bool,
}

impl Observer {
    fn synchronize(&mut self, channel: &CellularImsSipChannel) {
        if self.revoked { return; }
        let binding = channel.reg_event_binding();
        if self.binding != binding {
            self.subscription = binding.clone()
                .and_then(|value| RegEventSubscription::new(value, Instant::now()).ok());
            self.binding = binding;
        }
    }
    fn disable(&mut self) {
        if let Some(subscription) = self.subscription.as_mut() { subscription.disable(); }
    }
}

async fn owner_valid(session: &mut CellularImsLiveSession, runtime: &CellularImsRuntime) -> Result<(), ImsError> {
    ensure_worker_binding_current_ims(&session.worker_binding)?;
    if !runtime.task_is_current() { return Err(ImsError::new(code::RUNTIME_NOT_RUNNING)); }
    verify_mm_task_bearer(runtime, session.native_bearer.as_mut()).await.map_err(to_ims_error)?;
    verify_ims_card_binding(&session.device, session.auth_binding.as_ref()).await?;
    ensure_worker_binding_current_ims(&session.worker_binding)?;
    if !runtime.task_is_current() { return Err(ImsError::new(code::RUNTIME_NOT_RUNNING)); }
    Ok(())
}

fn request(session: &mut CellularImsLiveSession, authorization: Option<&str>) -> Result<Vec<u8>, ImsError> {
    let mut headers = Vec::new();
    if let Some(value) = session.channel.security_verify() {
        headers.push(SipHeader::new("Security-Verify", value));
    }
    // Do not invent serving-cell data merely to subscribe to registration state.
    session.reg_event.subscription.as_mut().ok_or_else(|| ImsError::new("ims_reg_event_unavailable"))?
        .build_request(&session.channel.route(), &session.registration, session.profile.ims.user_agent,
            &headers, authorization, Instant::now())
}

pub(super) async fn poll(live: &CellularImsLiveHandle, runtime: &CellularImsRuntime) {
    let mut sessions = live.session.lock().await;
    let Some(session) = sessions.as_mut() else { return; };
    if session.reg_event.revoked { return; }
    session.reg_event.synchronize(&session.channel);
    let action = session.reg_event.subscription.as_mut()
        .map(|subscription| subscription.poll(Instant::now())).unwrap_or(RegEventAction::Idle);
    if matches!(action, RegEventAction::Idle) { return; }
    if !tokio::time::timeout(Duration::from_secs(8), owner_valid(session, runtime)).await
        .is_ok_and(|result| result.is_ok())
    {
        session.reg_event.disable();
        return;
    }
    let wire = match action {
        RegEventAction::Begin => request(session, None),
        RegEventAction::Retransmit(wire) => Ok(wire),
        RegEventAction::Idle => return,
    };
    let result = match wire {
        Ok(wire) => session.channel.send_sip(&wire).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        session.reg_event.disable();
        tracing::debug!(reason = error.code(), "Cellular registration-event subscription unavailable");
    }
}

async fn authorize(
    session: &CellularImsLiveSession,
    challenge: &crate::connectivity::core::digest_aka::DigestChallenge,
    uri: &str,
) -> Result<String, ImsError> {
    // Stale reuse without a subscription-specific proof cache must not replay
    // a UICC vector or borrow REGISTER's credentials. Observation may stop.
    if challenge.stale { return Err(ImsError::new("ims_reg_event_auth_unavailable")); }
    let vector = digest_aka::decode_aka_nonce(&challenge.nonce).map_err(to_ims_error)?;
    let device = session.device.clone();
    let aid = session.aka_aid.clone();
    let aka = tokio::task::spawn_blocking(move || identity::run_usim_aka(
        QMI_PROXY_SOCKET, &device.qmi_device, device.uim_slot, &aid, &vector.rand, &vector.autn,
        2, Duration::from_secs(5), Duration::from_millis(300),
    )).await.map_err(|_| ImsError::new(code::USIM_AKA_FAILED))?.map_err(to_ims_error)?;
    // No AUTS-only subscription is ever marked authenticated on a bare 2xx.
    if aka.auts.is_some() { return Err(ImsError::new("ims_reg_event_auth_unavailable")); }
    let cnonce = sip::hex_token(8);
    let proof = digest_aka::compute_aka_response(
        &session.registration_identity.private_user, &challenge.realm, &aka,
        &challenge.algorithm, "SUBSCRIBE", uri, &challenge.nonce,
        challenge.qop.as_deref(), &cnonce, "00000001",
    ).map_err(to_ims_error)?;
    Ok(digest_aka::build_authorization_header(challenge, &session.registration_identity.private_user,
        uri, &proof, &cnonce, "00000001"))
}

pub(super) async fn on_frame(
    live: &CellularImsLiveHandle, runtime: &Arc<CellularImsRuntime>, frame: &[u8],
) -> Result<bool, CellularImsError> {
    let mut sessions = live.session.lock().await;
    let Some(session) = sessions.as_mut() else { return Ok(false); };
    // The lock prevents channel replacement while identity validation awaits.
    // Do not let a stale owner pin dialog/version state, even on a malformed frame.
    let relevant = classify_unsubscribed_notify(frame).is_some()
        || session.reg_event.subscription.as_ref().is_some_and(|subscription| subscription.wants_frame(frame));
    if !relevant { return Ok(false); }
    if !tokio::time::timeout(Duration::from_secs(8), owner_valid(session, runtime)).await
        .is_ok_and(|result| result.is_ok())
    {
        session.reg_event.disable();
        return Ok(classify_unsubscribed_notify(frame).is_some());
    }
    let classified = match session.reg_event.subscription.as_mut() {
        Some(subscription) => subscription.on_frame(frame, Instant::now()),
        None => match classify_unsubscribed_notify(frame) {
            Some(response_status) => RegEventFrame::Notify { response_status, effect: RegEventEffect::None },
            None => RegEventFrame::Other,
        },
    };
    match classified {
        RegEventFrame::Other => Ok(false),
        RegEventFrame::Response { .. } => Ok(true),
        RegEventFrame::AuthChallenge { challenge } => {
            if session.reg_event.revoked { return Ok(true); }
            let uri = session.reg_event.subscription.as_ref().expect("challenge subscription")
                .request_uri().to_owned();
            let authorization = tokio::time::timeout(Duration::from_secs(12), authorize(session, &challenge, &uri))
                .await.ok().and_then(Result::ok);
            if authorization.is_none() || !tokio::time::timeout(Duration::from_secs(8), owner_valid(session, runtime))
                .await.is_ok_and(|result| result.is_ok())
            {
                session.reg_event.disable();
                return Ok(true);
            }
            let sent = match request(session, authorization.as_deref()) {
                Ok(wire) => session.channel.send_sip(&wire).await,
                Err(error) => Err(error),
            };
            if sent.is_err() { session.reg_event.disable(); }
            Ok(true)
        }
        RegEventFrame::Notify { response_status, effect } => {
            let reason = match response_status {
                200 => "OK", 400 => "Bad Request", 415 => "Unsupported Media Type",
                420 => "Bad Extension", 489 => "Bad Event", 500 => "Server Internal Error",
                _ => "Call/Transaction Does Not Exist",
            };
            let response = sip::build_response(frame, response_status, reason, None, None, None);
            let sent = session.channel.send_sip(&response).await;
            if let RegEventEffect::BindingTerminated { reason, retry_after } = effect {
                if !session.reg_event.revoked {
                    session.reg_event.revoked = true;
                    session.channel.invalidate_registration();
                    session.mwi_subscription = None;
                    live.operator.set_ready(false);
                    let admission = session.endpoint_retries.lock().await.observe_registration_event(
                        session.pcscf, retry_after, Instant::now(),
                    );
                    let wait = live_retry::admission_error(admission, Instant::now());
                    let failure = CellularImsError::with_detail(code::REG_EVENT_BINDING_TERMINATED, reason.as_str());
                    runtime.update(|state| {
                        state.phase = CellularImsPhase::Degraded;
                        state.last_error = Some(failure.to_string());
                        state.last_failure_at = Some(now());
                    }).await;
                    if let Some(supplementary) = live.supplementary_runtime() {
                        supplementary.clear_registration(ImsRegistrationAccess::CellularIms).await;
                    }
                    tracing::info!(reason = reason.as_str(), waiting = wait.is_some(),
                        "Cellular own registration removed by validated reg-event");
                }
            }
            sent.map_err(map_channel_error)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
#[path = "reg_event_tests.rs"]
mod tests;
