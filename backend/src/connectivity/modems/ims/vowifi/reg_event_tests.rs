//! Production socket-owner N5 tests; loopback only, run ONLY in Actions.
use super::*;
use crate::connectivity::core::{context::SipTransport, ImsError};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};

const AOR: &str = "sip:registered@ims.example.test";
const ALIAS: &str = "sip:business-alias@ims.example.test";
const INSTANCE: &str = "urn:uuid:00000000-0000-4000-8000-000000000001";
const VERIFY: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc";

struct Provider {
    valid: AtomicBool,
    lost: AtomicBool,
    challenges: AtomicUsize,
    refreshes: AtomicUsize,
    retry: std::sync::Mutex<Option<u32>>,
    auth: bool,
    cancel_auth: bool,
    request: Vec<u8>,
}
impl RegisteredUnregister for Provider {
    fn reg_event_owner_valid<'a>(&'a self) -> futures_util::future::BoxFuture<'a, Result<(), ImsError>> {
        Box::pin(async move { if self.valid.load(Ordering::SeqCst) { Ok(()) }
            else { Err(ImsError::new("ims_uicc_binding_changed")) } })
    }
    fn reg_event_authorize<'a>(&'a self, challenge: &'a crate::connectivity::core::digest_aka::DigestChallenge,
        uri: &'a str) -> futures_util::future::BoxFuture<'a, Result<String, ImsError>> {
        Box::pin(async move {
            self.challenges.fetch_add(1, Ordering::SeqCst);
            if self.cancel_auth { self.valid.store(false, Ordering::SeqCst); }
            if !self.auth { return Err(ImsError::new("ims_reg_event_auth_unavailable")); }
            // Safe synthetic proof injection at the provider boundary. Never UICC.
            let cnonce = "synthetic-n5";
            let response = crate::connectivity::core::digest_aka::compute_digest_response(
                "synthetic-user", &challenge.realm, &[0x22; 8], &challenge.algorithm,
                "SUBSCRIBE", uri, &challenge.nonce, challenge.qop.as_deref(), cnonce, "00000001")?;
            Ok(crate::connectivity::core::digest_aka::build_authorization_header(
                challenge, "synthetic-user", uri, &response, cnonce, "00000001"))
        })
    }
    fn reg_event_terminated<'a>(&'a self, retry: Option<u32>) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async move { *self.retry.lock().unwrap() = retry; self.lost.store(true, Ordering::SeqCst); })
    }
    fn refresh<'a>(&'a self, channel: &'a mut SipChannel)
        -> futures_util::future::BoxFuture<'a, Result<RegisteredImsContext, ImsError>> {
        Box::pin(async move {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            let request = String::from_utf8(self.request.clone()).unwrap().replace("CSeq: 1 REGISTER", "CSeq: 2 REGISTER")
                .replace("branch=z9hG4bKregister", "branch=z9hG4bKrefresh");
            channel.send_sip(request.as_bytes()).await?;
            let response = channel.recv_sip(Duration::from_secs(2)).await?;
            let artifacts = channel.outbound_registered(&response, 600)?;
            Ok(RegisteredImsContext::from_artifacts(ImsRegistrationAccess::Vowifi, artifacts, 600))
        })
    }
    fn initial_request(&self) -> Result<Vec<u8>, ImsError> { Err(ImsError::new("test_unregister_unavailable")) }
    fn authenticated_request<'a>(&'a self, _: &'a [u8], _: u32)
        -> futures_util::future::BoxFuture<'a, Result<Vec<u8>, ImsError>> {
        Box::pin(async { Err(ImsError::new("test_unregister_unavailable")) })
    }
}

struct Fixture {
    line: String,
    context: RegisteredVoiceContext,
    channel: SipChannel,
    peer: TcpStream,
    pending: Vec<u8>,
    registered: Vec<u8>,
    provider: Arc<Provider>,
}
async fn read_frame(peer: &mut TcpStream, pending: &mut Vec<u8>) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(len) = sip_frame::complete_frame_len(pending) { return pending.drain(..len).collect(); }
            let mut bytes = [0; 4096];
            let len = peer.read(&mut bytes).await.unwrap();
            assert_ne!(len, 0, "loopback SIP owner closed");
            pending.extend_from_slice(&bytes[..len]);
        }
    }).await.expect("loopback SIP frame")
}
fn response(request: &[u8], status: u16, extra: &str) -> Vec<u8> {
    format!("SIP/2.0 {status} Test\r\nVia: {}\r\nFrom: {}\r\nTo: {};tag=network\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: 0\r\n\r\n",
        sip_frame::header_value(request, "Via").unwrap(), sip_frame::header_value(request, "From").unwrap(),
        sip_frame::header_value(request, "To").unwrap().split(";tag=").next().unwrap(),
        sip_frame::header_value(request, "Call-ID").unwrap(), sip_frame::header_value(request, "CSeq").unwrap()).into_bytes()
}
fn register_ok(request: &[u8]) -> Vec<u8> {
    response(request, 200, &format!("Contact: {}\r\nExpires: 600\r\nService-Route: <sip:service.ims.example.test;lr>\r\nP-Associated-URI: <{ALIAS}>\r\n",
        sip_frame::header_value(request, "Contact").unwrap()))
}
async fn fixture(auth: bool, cancel_auth: bool) -> Fixture {
    fixture_for_line(auth, cancel_auth, None).await
}
async fn fixture_for_line(auth: bool, cancel_auth: bool, line: Option<String>) -> Fixture {
    static ID: AtomicUsize = AtomicUsize::new(0);
    let line = line.unwrap_or_else(|| format!("n5-wifi-operator-{}", ID.fetch_add(1, Ordering::SeqCst)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    let route = ImsRoute { local_addr: client.local_addr().unwrap(), pcscf_addr: peer.local_addr().unwrap(), transport: SipTransport::Tcp };
    let request = format!("REGISTER sip:ims.example.test SIP/2.0\r\nVia: SIP/2.0/TCP {};branch=z9hG4bKregister\r\nFrom: <{AOR}>;tag=registration\r\nTo: <{AOR}>\r\nCall-ID: register-{line}\r\nCSeq: 1 REGISTER\r\nContact: <sip:flow@{};transport=tcp>\r\nExpires: 600\r\nContent-Length: 0\r\n\r\n", route.local_addr, route.local_addr).into_bytes();
    let mut channel = SipChannel::Tcp(EpdgSipChannel::new(client, vec![], route, Some(VERIFY.into())));
    channel.configure_outbound(&line, INSTANCE, true);
    assert!(channel.reg_event_binding().is_none());
    channel.send_sip(&request).await.unwrap();
    let mut pending = vec![];
    let registered = read_frame(&mut peer, &mut pending).await;
    peer.write_all(&register_ok(&registered)).await.unwrap();
    let accepted = channel.recv_sip(Duration::from_secs(2)).await.unwrap();
    let artifacts = channel.outbound_registered(&accepted, 600).unwrap();
    assert!(channel.reg_event_binding().is_some(), "must capture accepted FINAL REGISTER, not estimates");
    let provider = Arc::new(Provider { valid: AtomicBool::new(true), lost: AtomicBool::new(false),
        challenges: AtomicUsize::new(0), refreshes: AtomicUsize::new(0), retry: std::sync::Mutex::new(None), auth, cancel_auth, request });
    let context = RegisteredVoiceContext {
        line_id: line.clone(), profile_id: "n5-synthetic", identity: ImsIdentity { private_user: "synthetic-user".into(),
            public_uri: ALIAS.into(), contact_user: "flow".into(), home_domain: "ims.example.test".into(), contact_user_phone: false },
        route, registration: RegisteredImsContext::from_artifacts(ImsRegistrationAccess::Vowifi, artifacts, 600),
        security_verify: Some(VERIFY.into()), pani: "IEEE-802.11".into(), user_agent: "SimAdmin-N5-test".into(),
        expires_at: Instant::now() + Duration::from_secs(600), tcp_keepalive_interval: None, options_ping_interval: None,
        unregister: Some(provider.clone()), media_route_installer: None, media_interface: None,
        media_operator_creator: crate::connectivity::core::media::test_operator_socket_creator(),
    };
    Fixture { line, context, channel, peer, pending, registered, provider }
}
fn contact_xml(registered: &[u8], state: &str, event: &str, retry: &str) -> String {
    let contact = sip_frame::header_value(registered, "Contact").unwrap();
    let uri = contact.split('<').nth(1).unwrap().split('>').next().unwrap();
    let reg_id = crate::connectivity::core::ims_access::ImsAccess::Wlan.reg_id();
    format!("<contact id=\"c1\" state=\"{state}\" event=\"{event}\" {retry}><uri>{uri}</uri><unknown-param name=\"+sip.instance\">&lt;{INSTANCE}&gt;</unknown-param><unknown-param name=\"reg-id\">{reg_id}</unknown-param></contact>")
}
fn xml(version: u32, mode: &str, aor: &str, contact: &str) -> String {
    format!("<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"{mode}\"><registration id=\"r1\" aor=\"{aor}\" state=\"active\">{contact}</registration></reginfo>")
}
fn notify(subscribe: &[u8], cseq: u32, body: &str) -> Vec<u8> {
    let contact = sip_frame::header_value(subscribe, "Contact").unwrap();
    let target = contact.split('<').nth(1).unwrap().split('>').next().unwrap();
    format!("NOTIFY {target} SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1:5060;branch=z9hG4bKnotify{cseq}\r\nFrom: <{AOR}>;tag=network\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {cseq} NOTIFY\r\nContact: <sip:notifier@ims.example.test>\r\nEvent: reg\r\nSubscription-State: active;expires=600\r\nContent-Type: application/reginfo+xml\r\nContent-Length: {}\r\n\r\n{body}",
        sip_frame::header_value(subscribe, "From").unwrap(), sip_frame::header_value(subscribe, "Call-ID").unwrap(), body.len()).into_bytes()
}
async fn establish(peer: &mut TcpStream, pending: &mut Vec<u8>) -> Vec<u8> {
    let subscribe = read_frame(peer, pending).await;
    assert_eq!(sip_frame::header_value(&subscribe, "Event").as_deref(), Some("reg"));
    peer.write_all(&response(&subscribe, 200, "Contact: <sip:notifier@ims.example.test>\r\nExpires: 600\r\n")).await.unwrap();
    subscribe
}
async fn send_notify(peer: &mut TcpStream, pending: &mut Vec<u8>, notify: &[u8], status: u16) {
    peer.write_all(notify).await.unwrap();
    let ack = read_frame(peer, pending).await;
    assert_eq!(sip_frame::parse_status(&ack).unwrap(), status);
    assert_eq!(sip_frame::header_value(&ack, "CSeq"), sip_frame::header_value(notify, "CSeq"));
}
async fn wait_loss(provider: &Provider) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !provider.lost.load(Ordering::SeqCst) { tokio::task::yield_now().await; }
    }).await.unwrap();
}

#[tokio::test]
async fn automatic_subscribe_exact_register_aor_same_channel_route_and_security() {
    let Fixture { line, context, channel, mut peer, mut pending, .. } = fixture(false, false).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    assert!(subscribe.starts_with(format!("SUBSCRIBE {AOR} SIP/2.0").as_bytes()));
    assert!(!String::from_utf8_lossy(&subscribe).contains(ALIAS));
    assert_eq!(sip_frame::header_value(&subscribe, "Security-Verify").as_deref(), Some(VERIFY));
    assert_eq!(sip_frame::header_value(&subscribe, "Route").as_deref(), Some("<sip:service.ims.example.test;lr>"));
    assert_eq!(sip_frame::header_value(&subscribe, "P-Access-Network-Info").as_deref(), Some("IEEE-802.11"));
    assert!(sip_frame::header_value(&subscribe, "Authorization").is_none());
    disconnect_line(&line).await;
}

#[tokio::test]
async fn matching_full_termination_acks_then_latches_readiness_and_queued_refresh() {
    let Fixture { line, context, channel, mut peer, mut pending, registered, provider } = fixture(false, false).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    let body = xml(0, "full", AOR, &contact_xml(&registered, "terminated", "probation", "retry-after=\"120\""));
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 1, &body), 200).await;
    wait_loss(&provider).await;
    assert!(!operator_link_for_line(&line).is_available());
    assert_eq!(*provider.retry.lock().unwrap(), Some(120));
    let result = refresh_registered_for_line(&line, "n5-synthetic").await.unwrap().unwrap_err();
    assert_eq!(result.code(), reg_event::LOSS_REASON);
    assert_eq!(provider.refreshes.load(Ordering::SeqCst), 0);
    let options = b"OPTIONS sip:flow@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1;branch=z9hG4bKoptions\r\nFrom: <sip:peer@ims.example.test>;tag=remote\r\nTo: <sip:flow@ims.example.test>\r\nCall-ID: options-test\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n";
    peer.write_all(options).await.unwrap();
    assert_eq!(sip_frame::parse_status(&read_frame(&mut peer, &mut pending).await).unwrap(), 200);
    assert!(!operator_link_for_line(&line).is_available());
    assert_eq!(disconnect_line(&line).await, UnregisterResult::AlreadyExpired);
}

#[tokio::test]
async fn matching_partial_termination_after_full_revokes_only_own_line() {
    let Fixture { line, context, channel, mut peer, mut pending, registered, provider } = fixture(false, false).await;
    let other = fixture(false, false).await;
    let other_line = other.line.clone();
    install_registered_channel(other.context, other.channel).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    let active = xml(0, "full", AOR, &contact_xml(&registered, "active", "registered", ""));
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 1, &active), 200).await;
    let terminated = xml(1, "partial", AOR, &contact_xml(&registered, "terminated", "deactivated", ""));
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 2, &terminated), 200).await;
    wait_loss(&provider).await;
    assert!(operator_link_for_line(&other_line).is_available());
    disconnect_line(&line).await;
    disconnect_line(&other_line).await;
}

#[tokio::test]
async fn wrong_dialog_other_aor_instance_and_reg_id_never_revoke() {
    let Fixture { line, context, channel, mut peer, mut pending, registered, provider } = fixture(false, false).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    let contact = contact_xml(&registered, "terminated", "expired", "");
    let body = xml(0, "full", AOR, &contact);
    let wrong = String::from_utf8(notify(&subscribe, 1, &body)).unwrap().replace(";tag=network", ";tag=wrong");
    send_notify(&mut peer, &mut pending, wrong.as_bytes(), 481).await;
    let reg_id = crate::connectivity::core::ims_access::ImsAccess::Wlan.reg_id();
    for (version, aor, contact) in [(0, ALIAS, contact.clone()),
        (1, AOR, contact.replace(INSTANCE, "urn:uuid:00000000-0000-4000-8000-000000000002")),
        (2, AOR, contact.replace(&format!("name=\"reg-id\">{reg_id}"), "name=\"reg-id\">99"))] {
        send_notify(&mut peer, &mut pending, &notify(&subscribe, version + 2, &xml(version, "full", aor, &contact)), 200).await;
        assert!(!provider.lost.load(Ordering::SeqCst));
        assert!(operator_link_for_line(&line).is_available());
    }
    disconnect_line(&line).await;
}

#[tokio::test]
async fn version_gap_requests_full_without_revocation() {
    let Fixture { line, context, channel, mut peer, mut pending, registered, provider } = fixture(false, false).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 1, &xml(0, "full", AOR,
        &contact_xml(&registered, "active", "registered", ""))), 200).await;
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 2, &xml(2, "partial", AOR,
        &contact_xml(&registered, "terminated", "deactivated", ""))), 200).await;
    let full = read_frame(&mut peer, &mut pending).await;
    assert_eq!(sip_frame::header_value(&full, "Event").as_deref(), Some("reg"));
    assert!(!provider.lost.load(Ordering::SeqCst));
    assert!(operator_link_for_line(&line).is_available());
    disconnect_line(&line).await;
}

fn challenge(request: &[u8], proxy: bool) -> Vec<u8> {
    response(request, if proxy { 407 } else { 401 }, &format!("{}: Digest realm=\"ims.example.test\", nonce=\"ERERERERERERERERERERERERERERERERERERERERERE=\", algorithm=AKAv1-MD5, qop=\"auth\"\r\n",
        if proxy { "Proxy-Authenticate" } else { "WWW-Authenticate" }))
}

#[tokio::test]
async fn subscribe_challenges_bounded_two_fresh_method_correct_proofs() {
    for proxy in [false, true] {
        let Fixture { line, context, channel, mut peer, mut pending, provider, .. } = fixture(true, false).await;
        install_registered_channel(context, channel).await;
        let mut subscribe = read_frame(&mut peer, &mut pending).await;
        for _ in 0..2 {
            peer.write_all(&challenge(&subscribe, proxy)).await.unwrap();
            subscribe = read_frame(&mut peer, &mut pending).await;
            let header = sip_frame::header_value(&subscribe, if proxy { "Proxy-Authorization" } else { "Authorization" }).unwrap();
            assert!(header.contains("nc=00000001"));
            assert!(header.contains(&format!("uri=\"{AOR}\"")));
        }
        peer.write_all(&challenge(&subscribe, proxy)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(provider.challenges.load(Ordering::SeqCst), 2);
        assert!(operator_link_for_line(&line).is_available());
        assert!(!provider.lost.load(Ordering::SeqCst));
        disconnect_line(&line).await;
    }
}

#[tokio::test]
async fn unsupported_subscription_and_security_challenge_preserve_registration() {
    for security in [false, true] {
        let Fixture { line, context, channel, mut peer, mut pending, provider, .. } = fixture(true, false).await;
        install_registered_channel(context, channel).await;
        let subscribe = read_frame(&mut peer, &mut pending).await;
        let rejected = if security { String::from_utf8(challenge(&subscribe, false)).unwrap()
            .replace("Content-Length:", &format!("Security-Server: {VERIFY}\r\nContent-Length:")).into_bytes() }
            else { response(&subscribe, 489, "") };
        peer.write_all(&rejected).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(provider.challenges.load(Ordering::SeqCst), 0);
        assert!(operator_link_for_line(&line).is_available());
        disconnect_line(&line).await;
    }
}

#[tokio::test]
async fn owner_change_during_authentication_never_writes_continuation() {
    let Fixture { line, context, channel, mut peer, mut pending, provider, .. } = fixture(true, true).await;
    install_registered_channel(context, channel).await;
    let subscribe = read_frame(&mut peer, &mut pending).await;
    peer.write_all(&challenge(&subscribe, false)).await.unwrap();
    let mut bytes = [0; 1024];
    assert!(tokio::time::timeout(Duration::from_millis(200), peer.read(&mut bytes)).await.is_err());
    assert_eq!(provider.challenges.load(Ordering::SeqCst), 1);
    assert!(!provider.lost.load(Ordering::SeqCst));
    disconnect_line(&line).await;
}

#[tokio::test]
async fn natural_refresh_retains_subscription_version_and_replacement_resets_dialog() {
    let Fixture { line, context, channel, mut peer, mut pending, registered, provider } = fixture(false, false).await;
    install_registered_channel(context, channel).await;
    let subscribe = establish(&mut peer, &mut pending).await;
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 1, &xml(0, "full", AOR,
        &contact_xml(&registered, "active", "registered", ""))), 200).await;
    let refresh_line = line.clone();
    let refresh = tokio::spawn(async move { refresh_registered_for_line(&refresh_line, "n5-synthetic").await });
    let register = read_frame(&mut peer, &mut pending).await;
    assert!(sip_frame::is_request(&register, "REGISTER"));
    peer.write_all(&register_ok(&register)).await.unwrap();
    refresh.await.unwrap().unwrap().unwrap();
    assert_eq!(provider.refreshes.load(Ordering::SeqCst), 1);
    // Same accepted Contact keeps the old reg dialog and full snapshot.
    send_notify(&mut peer, &mut pending, &notify(&subscribe, 2, &xml(1, "partial", AOR,
        &contact_xml(&registered, "active", "refreshed", ""))), 200).await;
    let mut replacement = fixture_for_line(false, false, Some(line.clone())).await;
    install_registered_channel(replacement.context, replacement.channel).await;
    let new_subscribe = establish(&mut replacement.peer, &mut replacement.pending).await;
    assert_ne!(sip_frame::header_value(&subscribe, "Call-ID"), sip_frame::header_value(&new_subscribe, "Call-ID"));
    disconnect_line(&line).await;
}

#[tokio::test]
async fn mwi_and_refer_dispatch_remain_separate_from_reg_event() {
    let Fixture { line, context, channel, mut peer, mut pending, provider, .. } = fixture(false, false).await;
    let runtime = Arc::new(SupplementaryRuntime::for_line(&line));
    bind_supplementary_for_line(&line, runtime.clone());
    install_registered_channel(context, channel).await;
    let _reg = establish(&mut peer, &mut pending).await;
    let mwi = read_frame(&mut peer, &mut pending).await;
    assert_eq!(sip_frame::header_value(&mwi, "Event").as_deref(), Some("message-summary"));
    peer.write_all(&response(&mwi, 200, "")).await.unwrap();
    let body = "Messages-Waiting: yes\r\nVoice-Message: 1/0\r\n";
    let mwi_notify = String::from_utf8(notify(&mwi, 1, body)).unwrap()
        .replace("Event: reg", "Event: message-summary").replace("application/reginfo+xml", "application/simple-message-summary");
    send_notify(&mut peer, &mut pending, mwi_notify.as_bytes(), 200).await;
    assert!(runtime.snapshot().await.message_waiting.unwrap().messages_waiting);
    let refer = String::from_utf8(notify(&mwi, 2, "")).unwrap()
        .replace("Event: reg", "Event: refer;id=5").replace("application/reginfo+xml", "message/sipfrag");
    send_notify(&mut peer, &mut pending, refer.as_bytes(), 481).await;
    assert!(!provider.lost.load(Ordering::SeqCst));
    disconnect_line(&line).await;
}
