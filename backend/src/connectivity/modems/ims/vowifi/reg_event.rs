//! Same-owner reg-event glue. XML, dialogs, versioning and budgets live in core.
use super::*;
use crate::connectivity::core::{
    reg_event::{RegEventAction, RegEventBinding, RegEventEffect, RegEventFrame, RegEventSubscription},
    ImsError,
};

pub(super) const LOSS_REASON: &str = "ims_reg_event_binding_terminated";

#[derive(Default)]
pub(super) struct Observer {
    binding: Option<RegEventBinding>,
    subscription: Option<RegEventSubscription>,
    pub(super) revoked: bool,
}

impl Observer {
    fn synchronize(&mut self, channel: &SipChannel) {
        if self.revoked { return; }
        let binding = channel.reg_event_binding();
        if binding != self.binding {
            self.subscription = binding.clone().and_then(|binding| {
                RegEventSubscription::new(binding, Instant::now()).ok()
            });
            self.binding = binding;
        }
    }

    fn disable(&mut self) {
        if let Some(subscription) = self.subscription.as_mut() { subscription.disable(); }
    }
}

async fn owner_valid(session: &VoiceSession) -> bool {
    match session.context.unregister.as_ref() {
        Some(factory) => tokio::time::timeout(Duration::from_secs(8), factory.reg_event_owner_valid())
            .await.is_ok_and(|result| result.is_ok()),
        None => true,
    }
}

fn request(session: &mut VoiceSession, authorization: Option<&str>) -> Result<Vec<u8>, ImsError> {
    let mut headers = vec![SipHeader::new("P-Access-Network-Info", &session.context.pani)];
    if let Some(value) = session.context.security_verify.as_deref() {
        headers.push(SipHeader::new("Security-Verify", value));
    }
    session.reg_event.subscription.as_mut().ok_or_else(|| ImsError::new("ims_reg_event_unavailable"))?
        .build_request(&session.context.route, &session.context.registration,
            &session.context.user_agent, &headers, authorization, Instant::now())
}

pub(super) async fn poll(session: &mut VoiceSession) {
    if session.reg_event.revoked { return; }
    session.reg_event.synchronize(&session.channel);
    let action = session.reg_event.subscription.as_mut()
        .map(|subscription| subscription.poll(Instant::now())).unwrap_or(RegEventAction::Idle);
    if matches!(action, RegEventAction::Idle) { return; }
    if !owner_valid(session).await {
        session.reg_event.disable();
        return;
    }
    let wire = match action {
        RegEventAction::Begin => request(session, None),
        RegEventAction::Retransmit(wire) => Ok(wire),
        RegEventAction::Idle => return,
    };
    match wire {
        Ok(wire) if session.channel.send_sip(&wire).await.is_ok() => {},
        _ => session.reg_event.disable(),
    }
}

/// Always offered first. Even absent/unavailable subscriptions consume reg
/// NOTIFY via the shared strict classifier, never the generic SIP 200 path.
pub(super) async fn on_frame(session: &mut VoiceSession, link: &OperatorLink, frame: &[u8])
    -> Result<bool, String>
{
    let relevant = crate::connectivity::core::reg_event::classify_unsubscribed_notify(frame).is_some()
        || session.reg_event.subscription.as_ref().is_some_and(|subscription| subscription.wants_frame(frame));
    if !relevant { return Ok(false); }
    if !owner_valid(session).await {
        session.reg_event.disable();
        return Ok(true);
    }
    let classified = match session.reg_event.subscription.as_mut() {
        Some(subscription) => subscription.on_frame(frame, Instant::now()),
        None => match crate::connectivity::core::reg_event::classify_unsubscribed_notify(frame) {
            Some(response_status) => RegEventFrame::Notify { response_status, effect: RegEventEffect::None },
            None => RegEventFrame::Other,
        },
    };
    match classified {
        RegEventFrame::Other => Ok(false),
        RegEventFrame::Response { .. } => Ok(true),
        RegEventFrame::AuthChallenge { challenge } => {
            if session.reg_event.revoked { return Ok(true); }
            if !owner_valid(session).await {
                session.reg_event.disable();
                return Ok(true);
            }
            let uri = session.reg_event.subscription.as_ref().expect("challenge has subscription")
                .request_uri().to_string();
            let authorization = match session.context.unregister.as_ref() {
                Some(factory) => tokio::time::timeout(Duration::from_secs(8),
                    factory.reg_event_authorize(&challenge, &uri)).await.ok().and_then(Result::ok),
                None => None,
            };
            // The captured provider, not a mutable global profile, owns UICC.
            if !owner_valid(session).await {
                session.reg_event.disable();
                return Ok(true);
            }
            let wire = authorization.as_deref().and_then(|value| request(session, Some(value)).ok());
            match wire {
                Some(wire) if session.channel.send_sip(&wire).await.is_ok() => {},
                _ => session.reg_event.disable(),
            }
            Ok(true)
        },
        RegEventFrame::Notify { response_status, effect } => {
            if !owner_valid(session).await {
                session.reg_event.disable();
                return Ok(true);
            }
            let reason = match response_status {
                200 => "OK", 400 => "Bad Request", 415 => "Unsupported Media Type",
                420 => "Bad Extension", 500 => "Server Internal Error",
                489 => "Bad Event", _ => "Call/Transaction Does Not Exist",
            };
            let response = sip::build_response_for_access(frame, response_status, reason,
                None, None, None, &session.context.user_agent);
            // Acknowledge on the very same channel before changing readiness.
            let sent = session.channel.send_sip(&response).await;
            if let RegEventEffect::BindingTerminated { retry_after, .. } = effect {
                if !session.reg_event.revoked {
                    session.reg_event.revoked = true;
                    session.channel.invalidate_registration();
                    link.set_ready(false);
                    session.next_options_ping = None;
                    session.mwi_subscription = None;
                    if let Some(factory) = session.context.unregister.as_ref() {
                        factory.reg_event_terminated(retry_after).await;
                    }
                    if let Some(runtime) = session.supplementary.as_ref() {
                        runtime.clear_registration(ImsRegistrationAccess::Vowifi).await;
                    }
                }
            }
            sent.map_err(|error| error.code().to_string())?;
            Ok(true)
        },
    }
}
