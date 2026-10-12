//! Real cellular observer and socket-owner paths, with synthetic card/worker.
//! No hardware or namespace creation; executed only in GitHub Actions.
use super::*;
use tokio::net::UdpSocket;
use std::sync::atomic::{AtomicUsize, Ordering};

const AOR: &str = "sip:registered@ims.example.test";
const ALIAS: &str = "sip:business@ims.example.test";
const INSTANCE: &str = "urn:uuid:00000000-0000-4000-8000-000000000017";
const VERIFY: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc";

async fn receive(peer: &UdpSocket) -> Vec<u8> {
    let mut bytes = vec![0; 16384];
    let (n, _) = tokio::time::timeout(Duration::from_secs(3), peer.recv_from(&mut bytes)).await.unwrap().unwrap();
    bytes.truncate(n); bytes
}
fn response(request: &[u8], status: u16, extra: &str) -> Vec<u8> {
    format!("SIP/2.0 {status} Test\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=notifier\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: 0\r\n\r\n",
        sip::header_value(request, "Via").unwrap(), sip::header_value(request, "From").unwrap(),
        sip::header_value(request, "To").unwrap().split(";tag=").next().unwrap(),
        sip::header_value(request, "Call-ID").unwrap(), sip::header_value(request, "CSeq").unwrap()).into_bytes()
}
async fn fixture() -> (CellularImsLiveHandle, Arc<CellularImsRuntime>, UdpSocket, Vec<u8>) {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    let (live, runtime, peer) = super::super::tests::test_voice_session().await;
    let line = format!("n5-cellular-{}", IDS.fetch_add(1, Ordering::Relaxed));
    let mut sessions = live.session.lock().await;
    let session = sessions.as_mut().unwrap();
    let route = ImsRoute { local_addr: "127.0.0.1:0".parse().unwrap(), ..session.channel.route() };
    session.channel = CellularImsSipChannel::bind(route, None, Some(VERIFY.into())).unwrap();
    session.device.line_id = line.clone();
    session.registration_identity.public_uri = AOR.into();
    session.identity.public_uri = ALIAS.into();
    session.sip_instance = INSTANCE.into();
    session.endpoint_retries = live.retry_scope.lock().await.for_card(session.auth_binding.as_ref().unwrap());
    session.channel.configure_outbound(&line, INSTANCE, true);
    let route = session.channel.route();
    let request = format!("REGISTER sip:ims.example.test SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bKregister\r\nFrom: <{AOR}>;tag=reg\r\nTo: <{AOR}>\r\nCall-ID: {line}\r\nCSeq: 1 REGISTER\r\nContact: <sip:flow@{};transport=udp>\r\nExpires: 600\r\nContent-Length: 0\r\n\r\n", route.local_addr, route.local_addr).into_bytes();
    session.channel.send_sip(&request).await.unwrap();
    let wire = receive(&peer).await;
    let accepted = response(&wire, 200, &format!("Contact: {}\r\nExpires: 600\r\nService-Route: <sip:service.example.test;lr>\r\n",
        sip::header_value(&wire, "Contact").unwrap()));
    let artifacts = session.channel.outbound_registered(&accepted, 600).unwrap();
    session.registration = RegisteredImsContext::from_artifacts(ImsRegistrationAccess::CellularIms, artifacts, 600);
    assert!(session.channel.reg_event_binding().is_some());
    runtime.update(|state| { state.phase = CellularImsPhase::Registered; state.stage = CellularImsStage::Registered; }).await;
    live.operator.set_ready(true);
    drop(sessions);
    (live, runtime, peer, wire)
}
async fn establish(live: &CellularImsLiveHandle, runtime: &Arc<CellularImsRuntime>, peer: &UdpSocket) -> Vec<u8> {
    poll(live, runtime).await;
    let sub = receive(peer).await;
    let accepted = response(&sub, 200, "Contact: <sip:notifier@ims.example.test>\r\nExpires: 600\r\n");
    assert!(on_frame(live, runtime, &accepted).await.unwrap());
    sub
}
fn xml(registered: &[u8], version: u32, partial: bool, state: &str, event: &str, retry: &str) -> String {
    let contact = sip::header_value(registered, "Contact").unwrap();
    let uri = contact.split('<').nth(1).unwrap().split('>').next().unwrap();
    let id = crate::connectivity::core::ims_access::ImsAccess::Cellular.reg_id();
    format!("<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"{}\"><registration id=\"r\" aor=\"{AOR}\" state=\"active\"><contact id=\"c\" state=\"{state}\" event=\"{event}\" {retry}><uri>{uri}</uri><unknown-param name=\"+sip.instance\">&lt;{INSTANCE}&gt;</unknown-param><unknown-param name=\"reg-id\">{id}</unknown-param></contact></registration></reginfo>", if partial { "partial" } else { "full" })
}
fn notify(sub: &[u8], seq: u32, body: &str, state: &str) -> Vec<u8> {
    let contact = sip::header_value(sub, "Contact").unwrap();
    let uri = contact.split('<').nth(1).unwrap().split('>').next().unwrap();
    format!("NOTIFY {uri} SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKnotify{seq}\r\nFrom: <{AOR}>;tag=notifier\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {seq} NOTIFY\r\nContact: <sip:notifier@ims.example.test>\r\nEvent: reg\r\nSubscription-State: {state}\r\nContent-Type: application/reginfo+xml\r\nContent-Length: {}\r\n\r\n{body}",
        sip::header_value(sub, "From").unwrap(), sip::header_value(sub, "Call-ID").unwrap(), body.len()).into_bytes()
}
async fn deliver(live: &CellularImsLiveHandle, runtime: &Arc<CellularImsRuntime>, peer: &UdpSocket, frame: &[u8], status: u16) {
    assert!(on_frame(live, runtime, frame).await.unwrap());
    assert_eq!(sip::parse_status(&receive(peer).await).unwrap(), status);
}

#[tokio::test]
async fn cellular_reg_subscribe_uses_accepted_wire_binding_not_business_alias() {
    let (live, runtime, peer, _) = fixture().await;
    let sub = establish(&live, &runtime, &peer).await;
    assert!(sub.starts_with(format!("SUBSCRIBE {AOR} SIP/2.0").as_bytes()));
    assert!(!String::from_utf8_lossy(&sub).contains(ALIAS));
    assert_eq!(sip::header_value(&sub, "Security-Verify").as_deref(), Some(VERIFY));
    assert_eq!(sip::header_value(&sub, "Route").as_deref(), Some("<sip:service.example.test;lr>"));
    assert_eq!(sip::header_value(&sub, "Event").as_deref(), Some("reg"));
    assert!(sip::header_value(&sub, "Authorization").is_none());
    let before = sip::header_value(&sub, "Call-ID");
    poll(&live, &runtime).await;
    assert_eq!(before, sip::header_value(&sub, "Call-ID"));
}

#[tokio::test]
async fn cellular_reg_own_termination_latches_runtime_and_scope_wait_without_downgrade() {
    let (live, runtime, peer, wire) = fixture().await;
    let sub = establish(&live, &runtime, &peer).await;
    let removal = notify(&sub, 1, &xml(&wire, 0, false, "terminated", "probation", "retry-after=\"300\""), "active;expires=600");
    deliver(&live, &runtime, &peer, &removal, 200).await;
    assert!(!runtime.status().await.registered);
    assert!(!live.operator.is_available());
    assert!(runtime.status().await.last_error.unwrap().contains(code::REG_EVENT_BINDING_TERMINATED));
    assert!(live.live_xcap_access().await.is_none());
    let mut guard = live.session.lock().await;
    let session = guard.as_mut().unwrap();
    assert!(session.reg_event.revoked);
    assert_eq!(session.channel.security_verify(), Some(VERIFY));
    assert!(session.channel.reg_event_binding().is_none());
    let state = live.retry_scope.lock().await.for_card(session.auth_binding.as_ref().unwrap());
    assert!(state.lock().await.restart_admission(Instant::now()).remaining(Instant::now()).unwrap() > Duration::from_secs(290));
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let result = refresh_live_registration(session, &runtime, "reg-event-test", &db).await;
    assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::NetworkRejected));
    drop(guard);
    deliver(&live, &runtime, &peer, &removal, 200).await; // duplicate ack, no new loss/retry budget
    assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::AlreadyExpired);
    assert!(send_live_sms_for_line(&live, &runtime, "1234", "not sent", "1234").await.is_err());
}

#[tokio::test]
async fn cellular_reg_full_partial_and_version_gap_are_transactional() {
    let (live, runtime, peer, wire) = fixture().await;
    let sub = establish(&live, &runtime, &peer).await;
    deliver(&live, &runtime, &peer, &notify(&sub, 1, &xml(&wire, 0, false, "active", "registered", ""), "active;expires=600"), 200).await;
    deliver(&live, &runtime, &peer, &notify(&sub, 2, &xml(&wire, 2, true, "terminated", "expired", ""), "active;expires=600"), 200).await;
    assert!(live.operator.is_available());
    poll(&live, &runtime).await;
    let renewal = receive(&peer).await;
    assert_eq!(sip::header_value(&renewal, "Call-ID"), sip::header_value(&sub, "Call-ID"));
    deliver(&live, &runtime, &peer, &notify(&sub, 3, &xml(&wire, 2, false, "active", "registered", ""), "active;expires=600"), 200).await;
    deliver(&live, &runtime, &peer, &notify(&sub, 4, &xml(&wire, 3, true, "terminated", "deactivated", ""), "active;expires=600"), 200).await;
    assert!(!live.operator.is_available());
}

#[tokio::test]
async fn cellular_reg_wrong_dialog_identity_and_subscription_termination_do_not_revoke() {
    let (live, runtime, peer, wire) = fixture().await;
    let sub = establish(&live, &runtime, &peer).await;
    let body = xml(&wire, 0, false, "terminated", "expired", "");
    let wrong = String::from_utf8(notify(&sub, 1, &body, "active;expires=600")).unwrap().replace(";tag=notifier", ";tag=wrong");
    deliver(&live, &runtime, &peer, wrong.as_bytes(), 481).await;
    for (n, altered) in [body.replace(INSTANCE, "urn:uuid:00000000-0000-4000-8000-000000000018"), body.replace(AOR, ALIAS)].into_iter().enumerate() {
        deliver(&live, &runtime, &peer, &notify(&sub, n as u32 + 2, &altered, "active;expires=600"), 200).await;
        assert!(live.operator.is_available());
    }
    deliver(&live, &runtime, &peer, &notify(&sub, 4, "", "terminated;reason=timeout"), 200).await;
    assert!(live.operator.is_available());
    assert!(runtime.status().await.registered);
}

#[tokio::test]
async fn cellular_reg_unsupported_subscription_and_ownership_failures_only_disable_observation() {
    let (live, runtime, peer, wire) = fixture().await;
    poll(&live, &runtime).await;
    let sub = receive(&peer).await;
    assert!(on_frame(&live, &runtime, &response(&sub, 489, "")).await.unwrap());
    poll(&live, &runtime).await;
    assert!(live.operator.is_available());
    assert!(live.session.lock().await.as_ref().unwrap().reg_event.subscription.as_ref().unwrap().is_disabled());
    let body = xml(&wire, 0, false, "terminated", "expired", "");
    deliver(&live, &runtime, &peer, &notify(&sub, 1, &body, "active;expires=600"), 481).await;
    assert!(live.operator.is_available());
}

#[tokio::test]
async fn cellular_reg_unrelated_mwi_refer_and_no_binding_are_not_silently_accepted() {
    let (live, runtime, peer, wire) = fixture().await;
    let sub = establish(&live, &runtime, &peer).await;
    let frame = notify(&sub, 1, &xml(&wire, 0, false, "active", "registered", ""), "active;expires=600");
    for event in ["message-summary", "refer;id=5"] {
        let unrelated = String::from_utf8(frame.clone()).unwrap().replace("Event: reg", &format!("Event: {event}"));
        assert!(!on_frame(&live, &runtime, unrelated.as_bytes()).await.unwrap());
    }
    live.session.lock().await.as_mut().unwrap().reg_event = Observer::default();
    deliver(&live, &runtime, &peer, &frame, 481).await;
    assert!(live.operator.is_available());
}
