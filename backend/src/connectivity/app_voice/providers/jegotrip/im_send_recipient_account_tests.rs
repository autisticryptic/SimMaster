//! Actions-only actual AccountManager -> original bootstrap/SDK -> purpose-bound
//! auxiliary aidToUid -> original userSend -> SQL/cancel/true stop/physical Joins.
//! Child of im_read_status_recipient_account_tests; no received span is a grant.
use super::*;
use n::{
    account_config::AccountConfig, account_runtime::AccountManager,
    account_test_support::TempDatabase, im_inbox::{ImSendMessage, ImSendView},
    im_send_policy::NativeImSendConfig,
};
use crate::platform::db::Database;

const ALIAS: &str = "[phone:+12345@account-domain.invalid]";
const OTHER_ALIAS: &str = "[phone:+67890@account-domain.invalid]";
const UID_URI: &str = "[uid:1_2@account-domain.invalid]";
const SELECTOR: &[u8] = b"fixture-original-selector";
const CHANGED_SELECTOR: &[u8] = b"fixture-changed-selector";
const NONCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEXT_NONCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const THIRD_NONCE: &str = "cccccccccccccccccccccccccccccccc";
const FOURTH_NONCE: &str = "dddddddddddddddddddddddddddddddd";
const TEXT: &str = "private \"recipient\"\nIM 🥘";
const JSON_BODY: &str = "{\"text\":\"private \\\"recipient\\\"\\nIM 🥘\"}";
const INDEX: i64 = 4_294_967_297;
const ORIGINAL_TIME: i64 = -8_589_934_593;

fn send_configuration(root: &str, port: u16, changed_selector: bool) -> AccountConfig {
    let mut cfg = bootstrap_account_config(root, port);
    cfg.profile.incoming = None;
    cfg.profile.native_receivers = Some(n::native_receiver::NativeReceiverConfig {
        im_messages: Some(n::im_delivery::NativeImConfig { notification_timeout_ms: 3000 }),
        direct_domains: None, invite_notifications: None,
    });
    cfg.profile.client_ex = true;
    cfg.profile.im_sync = None;
    cfg.profile.im_receipts = None;
    cfg.profile.im_user_read = None;
    cfg.profile.im_read_status = None;
    cfg.profile.im_send = Some(NativeImSendConfig {
        send_text: true, send_info: true, auth_country_code: None,
    });
    assert!(cfg.reconnect.is_none(), "no SDK retry installation");
    if changed_selector {
        let mut private: serde_json::Value = serde_json::from_slice(&cfg.encode_private().unwrap()).unwrap();
        private["config"]["credentials"]["sdk"]["selected_session_id"] =
            serde_json::json!("fixture-changed-selector");
        cfg = AccountConfig::decode_private(&serde_json::to_vec(&private).unwrap()).unwrap();
    }
    assert_eq!(cfg.credentials.sdk.selected_session_id.as_ref().unwrap().expose_to_backend().as_bytes(),
        if changed_selector { CHANGED_SELECTOR } else { SELECTOR });
    assert!(cfg.profile.im_sync.is_none() && cfg.profile.im_receipts.is_none());
    assert!(cfg.profile.im_user_read.is_none() && cfg.profile.im_read_status.is_none());
    assert!(cfg.profile.incoming.is_none() && cfg.profile.client_ex);
    assert!(cfg.profile.native_receivers.as_ref().unwrap().im_messages.is_some());
    cfg
}
fn text() -> ImSendMessage { ImSendMessage::text(TEXT).unwrap() }
fn changed_text() -> ImSendMessage { ImSendMessage::text("changed private recipient body").unwrap() }

fn exact_map(actual: &[RpcProperty<'_>], expected: &[(&[u8], &[u8])]) {
    assert_eq!(actual.len(), expected.len(), "no extra authentication/business/map fields");
    for &(key, value) in expected {
        assert_eq!(actual.iter().filter(|p| p.key == key).count(), 1);
        assert_eq!(actual.iter().find(|p| p.key == key).unwrap().value, value);
    }
}
fn private_digest(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = ring::digest::Context::new(&ring::digest::SHA256);
    for p in parts { h.update(&(p.len() as u64).to_be_bytes()); h.update(p); }
    h.finish().as_ref().to_vec()
}
fn nonce_tag() -> String {
    let mut h = ring::digest::Context::new(&ring::digest::SHA256);
    h.update(b"original-im-send-ui-nonce-v1\0"); h.update(NONCE.as_bytes());
    h.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, PartialEq, Eq)]
struct SendRow {
    id: i64, revision: i64, identity: Vec<u8>, nonce: String, intent_hash: Vec<u8>,
    box_id: Option<Vec<u8>>, arguments_hash: Option<Vec<u8>>, kind: String,
    run: String, epoch: String, state: i64, request: Option<i32>, code: Option<i64>,
    index: Option<i64>, time: Option<i64>, updated_at: String,
}
fn send_row(db: &Database, line: &str) -> SendRow {
    db.with_conn(|c| c.query_row(
        "SELECT id,revision,identity_key,user_nonce,intent_hash,box_id,arguments_hash,message_kind,run_nonce,scope_epoch,state,request_id,result_code,message_index,original_time,updated_at FROM app_voice_im_outbox WHERE line_id=?1",
        [line], |r| Ok(SendRow {
            id: r.get(0)?, revision: r.get(1)?, identity: r.get(2)?, nonce: r.get(3)?,
            intent_hash: r.get(4)?, box_id: r.get(5)?, arguments_hash: r.get(6)?, kind: r.get(7)?,
            run: r.get(8)?, epoch: r.get(9)?, state: r.get(10)?, request: r.get(11)?,
            code: r.get(12)?, index: r.get(13)?, time: r.get(14)?, updated_at: r.get(15)?,
        }),
    ).map_err(|_| "original SEND SQL snapshot")).unwrap()
}
fn count_rows(db: &Database, table: &str, line: &str) -> i64 {
    db.with_conn(|c| c.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE line_id=?1"),
        [line], |r| r.get(0)).map_err(|_| "original account row count")).unwrap()
}
fn marker(row: &SendRow, selector: &[u8]) {
    assert!(row.id > 0); assert_eq!(row.revision, 1); assert_eq!(row.nonce, NONCE);
    assert_eq!(row.identity, private_digest(&[b"jegotrip-native-im-context-v1\0", b"fixture-user", selector]));
    assert_eq!(row.intent_hash.len(), 32); assert_eq!(row.kind, "text");
    assert_eq!(row.run.len(), 32); assert_eq!(row.epoch.len(), 32);
    assert!(!row.updated_at.is_empty() && row.updated_at.len() <= 64);
}
fn no_send_permit(row: &SendRow, state: i64) {
    assert_eq!(row.state, state);
    assert_eq!((row.request, row.code, row.index, row.time), (None, None, None, None));
    assert_eq!(row.box_id, None, "a plain/returned UID is not a private SEND grant");
    assert_eq!(row.arguments_hash, None, "no private WRITE arguments or SEND ID before purpose resolution");
}
fn no_other_business(manager: &AccountManager, db: &Database, line: &str) {
    for table in [
        "app_voice_im_messages", "app_voice_im_receive_cursors", "app_voice_im_read_receipts",
        "app_voice_im_user_reads", "app_voice_im_read_status_actions", "app_voice_im_read_status_targets",
        "app_voice_inbox", "app_voice_offline_cursors",
    ] {
        assert_eq!(count_rows(db, table, line), 0, "SEND needs no receivedbody/sync/UDMR/SMS: {table}");
    }
    assert_eq!(db.with_conn(|c| c.query_row("SELECT COUNT(*) FROM sms_messages", [],
        |r| r.get::<_, i64>(0)).map_err(|_| "SMS fixture count")).unwrap(), 0);
    assert!(manager.im_messages(line, 16).unwrap().is_empty());
    assert!(manager.messages(line, 16).unwrap().is_empty());
    assert!(manager.im_user_read_history(line, 16).unwrap().is_empty());
    assert!(manager.im_read_status_history(line, 16).unwrap().is_empty());
    assert!(db.get_call_history_for_line(line, 16, 0).unwrap().is_empty());
    assert!(db.get_recent_app_events(Some(line), 16).unwrap().iter().all(|e| e.event_type != "im.received"));
}
fn lease_zero(db: &Database, line: &str) {
    assert_eq!(count_rows(db, "app_voice_runtime_leases", line), 0);
    assert!(!db.with_conn(|c| n::account_runs::ownership_pending(c, line)).unwrap());
}
fn public_view(view: &ImSendView, id: i64, state: &str, submitted_result: bool) {
    assert_eq!(view.id, id); assert_eq!(view.kind, "text"); assert_eq!(view.state, state);
    assert_eq!(view.nonce_tag, nonce_tag());
    assert_eq!(view.message_index.as_deref(), if submitted_result { Some("4294967297") } else { None });
    assert_eq!(view.original_time.as_deref(), if submitted_result { Some("-8589934593") } else { None });
    let value = serde_json::to_value(view).unwrap();
    assert_eq!(value.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["id", "kind", "message_index", "nonce_tag", "original_time", "state", "updated_at"]);
    let public = serde_json::to_string(view).unwrap();
    for private in [NONCE, TEXT, ALIAS, "12345", "fixture", "1_2", "P2P/", "UserEntry", "token",
        "intent_hash", "arguments_hash", "identity_key", "scope_epoch", "run_nonce", "Statistics."] {
        assert!(!public.contains(private), "history is only sanitized result DATA");
    }
}
async fn wait_phase(manager: &AccountManager, line: &str, expected: &str) {
    bounded(async { loop {
        if manager.get(line, 100).await.unwrap().phase == expected { break; }
        tokio::task::yield_now().await;
    }}).await;
}
async fn healthy(manager: &AccountManager, line: &str) {
    let v = manager.get(line, 100).await.unwrap();
    assert_eq!(v.phase, "hosted"); assert_eq!(v.im_send_supported, Some(true));
    assert_eq!(v.user_entry_supported, Some(true));
    assert_eq!(v.im_user_read_supported, Some(false));
    assert_eq!(v.im_read_status_supported, Some(false));
    assert!(!v.incoming_call_ready && !v.outbound_authorized);
}

async fn bootstrap_peer(listener: TcpListener, server: std::sync::Arc<tokio_rustls::rustls::ServerConfig>) {
    let mut peer = CommonPeer::open(listener, server).await;
    let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
    let r = decode_request(&packet[1..]).unwrap();
    assert_eq!(r.method, n::rpc_bootstrap::GET_APP_CONFIGS_METHOD.as_bytes());
    exact_map(&r.metadata, &[(b"id", b"BootEntry")]);
    let mut a = Reader::versioned(r.arguments).unwrap();
    assert_eq!(a.bytes().unwrap(), b"fixture-original-appkey"); a.finish().unwrap();
    peer.send(&[reply(r.call_id, &bootstrap_user_entry())]).await;
    peer.no_more_common().await;
}
async fn login_capture(peer: &mut CommonPeer, selector: &[u8]) -> Vec<u8> {
    let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
    let r = decode_request(&packet[1..]).unwrap();
    assert_eq!(r.method, n::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
    exact_map(&r.metadata, &[(b"id", b"DestEntry"), (b"pwd", b"fixture-original-pwd")]);
    let mut a = Reader::versioned(r.arguments).unwrap();
    // This is captured from the ACTUAL normalized login, not constructed from
    // the returned UID/mainAccount (both deliberately different DATA).
    let own_uri = a.bytes().unwrap().to_vec();
    assert_eq!(own_uri, b"[username:fixture-account@account-domain.invalid]");
    assert_eq!(a.bytes().unwrap(), selector);
    exact_map(&a.properties().unwrap(), &[
        (b"ClientId", b"fixture-exact-client-id"), (b"DeviceId", b"fixture-original-device"),
        (b"ForceFlag", b"0"), (b"__appKey", b"fixture-original-appkey"),
        (b"__domain", b"fixture-domain"), (b"__app", b"fixture-app"), (b"DeviceInfo.CC", b"fixture-cc"),
    ]);
    a.finish().unwrap();
    peer.send(&[reply(r.call_id, &login_result_for_account())]).await;
    let (uri, flag) = hosting_call(peer).await;
    assert_eq!(uri, own_uri); assert_eq!(flag, b"4");
    own_uri
}
fn strict_idle(packet: &[u8], selector: &[u8]) {
    if matches!(packet, [2] | [14]) { return; }
    assert_eq!(packet.first(), Some(&0), "no arbitrary control/reply on original idle SDK");
    let r = decode_request(&packet[1..]).unwrap();
    assert_eq!(r.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(),
        "no SEND/replay/refresh/recv/UDMR/setRead/SMS/call/logout on idle SDK");
    exact_map(&r.metadata, &[(b"id", b"DestEntry"), (b"token", b"fixture-auth")]);
    let mut a = Reader::versioned(r.arguments).unwrap();
    assert_eq!(a.bytes().unwrap(), b"fixture-user"); assert_eq!(a.bytes().unwrap(), selector);
    let batch = n::rpc_batch::read_payload(&mut a).unwrap();
    assert!(batch.requests.is_empty() && batch.results.is_empty(), "strict original empty poll only");
    a.finish().unwrap();
}
async fn active_idle(peer: &mut CommonPeer, packet: &[u8], selector: &[u8]) {
    strict_idle(packet, selector);
    if packet == [2] { peer.send(&[vec![14]]).await; }
    else if packet != [14] { poll(peer, &decode_request(&packet[1..]).unwrap()).await; }
}
async fn send_call(peer: &mut CommonPeer, own_uri: &[u8], db: &Database, line: &str) -> SendRow {
    let packet = loop {
        let packet = peer.next().await.unwrap();
        if packet.first() == Some(&0) {
            let r = decode_request(&packet[1..]).unwrap();
            if r.method == n::im_send_data::USER_SEND_METHOD.as_bytes() { break packet; }
        }
        active_idle(peer, &packet, SELECTOR).await;
    };
    let r = decode_request(&packet[1..]).unwrap();
    assert_eq!(r.method, b"userSend.Message.Message");
    exact_map(&r.metadata, &[(b"id", b"#Group"), (b"token", b"fixture-auth")]);
    assert_eq!(&r.arguments[..5], &[0, 0, 1, 0, 0]);
    let mut a = Reader::versioned(r.arguments).unwrap();
    assert_eq!(a.bytes().unwrap(), b"P2P/1_2"); assert_eq!(a.bytes().unwrap(), b"Text");
    assert_eq!(a.bytes().unwrap(), JSON_BODY.as_bytes());
    assert!(a.properties().unwrap().is_empty(), "no invented attachment");
    exact_map(&a.properties().unwrap(), &[(b"account", own_uri)]);
    assert_ne!(own_uri, b"fixture-user"); assert_ne!(own_uri, b"fixture-main-account");
    assert_ne!(own_uri, b"fixture-account");
    exact_map(&a.properties().unwrap(), &[
        (b"Notify.Text", TEXT.as_bytes()), (b"Statistics.AppId", b"fixture-app"),
        (b"Statistics.DomainId", b"fixture-domain"), (b"Statistics.CountryCode", b""),
    ]);
    a.finish().unwrap();
    let row = send_row(db, line); marker(&row, SELECTOR);
    assert_eq!((row.state, row.request, row.code, row.index, row.time), (0, Some(r.call_id), None, None, None));
    assert_eq!(row.box_id.as_deref(), Some(b"P2P/1_2".as_slice()));
    assert_eq!(row.arguments_hash.as_deref(), Some(private_digest(&[
        b"original-user-send-arguments-v1\0", r.arguments,
    ]).as_slice()));
    row
}
fn accepted_send_reply(id: i32) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(true).unwrap();
    w.i64(INDEX).unwrap(); w.i64(ORIGINAL_TIME).unwrap(); w.bytes(b"").unwrap();
    reply(id, w.finish().expose_to_transport())
}
async fn idle_until_finish(peer: &mut CommonPeer, selector: &[u8], mut finish: oneshot::Receiver<()>) {
    loop { tokio::select! { biased;
        v = &mut finish => { v.unwrap(); return; }
        packet = peer.next() => { active_idle(peer, &packet.unwrap(), selector).await; }
    }}
}
fn unregister_nested(method: &[u8], metadata: &[RpcProperty<'_>], arguments: &[u8], own_uri: &[u8]) {
    assert_eq!(method, n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
    exact_map(metadata, &[(b"id", b"#SipGateway")]);
    let mut a = Reader::versioned(arguments).unwrap(); assert_eq!(a.bytes().unwrap(), own_uri);
    exact_map(&a.properties().unwrap(), &[
        (b"SipFlag", b"3"), (b"AuthnamePrefix", b"fixture-prefix"), (b"ChannelId", b"fixture-channel"),
        (b"DeleteIntCode", b"1"), (b"SipNumber", b"fixture-number"), (b"SipPwd", b"fixture-SipPwd"),
    ]); a.finish().unwrap();
}
async fn finish_sdk(mut peer: CommonPeer, own_uri: &[u8], selector: &[u8]) {
    let mut unregistered = false;
    loop {
        let packet = peer.next().await.unwrap();
        if matches!(packet.as_slice(), [2] | [14]) { active_idle(&mut peer, &packet, selector).await; continue; }
        assert_eq!(packet[0], 0); let r = decode_request(&packet[1..]).unwrap();
        exact_map(&r.metadata, &[(b"id", b"DestEntry"), (b"token", b"fixture-auth")]);
        let mut a = Reader::versioned(r.arguments).unwrap();
        assert_eq!(a.bytes().unwrap(), b"fixture-user"); assert_eq!(a.bytes().unwrap(), selector);
        if r.method == n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes() {
            assert!(unregistered, "normal stop performs original unhosting before logout");
            a.finish().unwrap(); peer.send(&[reply(r.call_id, &[0, 1, 0, 0, 0, 0])]).await; break;
        }
        assert_eq!(r.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(), "no other business during true stop");
        let batch = n::rpc_batch::read_payload(&mut a).unwrap(); assert!(batch.results.is_empty());
        for nested in batch.requests.values() {
            unregister_nested(nested.method, &nested.metadata, nested.arguments, own_uri);
            unregistered = true;
        }
        a.finish().unwrap();
        // Reuse the original parent's ACK encoding, after strict input checks.
        poll(&mut peer, &r).await;
    }
    peer.no_more_common().await;
}
async fn retired_input_eof(peer: &mut CommonPeer) {
    // Full original scope retirement is INPUT ONLY. next() would emit Channel
    // ACK on the retired fd; no ACK/unhosting/logout/result is sent here.
    for _ in 0..128 {
        match peer.next_input_only().await {
            None => return,
            Some(packet) => strict_idle(&packet, SELECTOR),
        }
    }
    panic!("retired original input exceeded existing 128-record cap");
}
fn auxiliary_call(packet: &[u8]) -> i32 {
    assert_eq!(packet[0], 0); let r = decode_request(&packet[1..]).unwrap();
    assert_eq!(r.method, n::user_entry_data::AID_TO_UID_METHOD.as_bytes());
    exact_map(&r.metadata, &[(b"id", b"prefix/UserEntry"), (b"token", b"fixture-auth"), (b"DeviceInfo.CC", b"fixture-cc")]);
    let mut a = Reader::versioned(r.arguments).unwrap(); assert_eq!(a.u32().unwrap(), 1);
    assert_eq!(a.bytes().unwrap(), b"phone"); assert_eq!(a.bytes().unwrap(), b"+12345"); a.finish().unwrap();
    r.call_id
}
fn negative_auxiliary_reply(id: i32, variant: u8) -> Vec<u8> {
    match variant {
        0 => reply(id, &aid(2)), // false with an otherwise valid UID: never a grant
        1 => { let mut w = Writer::new(); w.boolean(true).unwrap(); w.u32(0).unwrap(); w.bytes(b"").unwrap(); reply(id, w.finish().expose_to_transport()) }
        2 => { let r = encode_reply(id, 65537, &[0]).unwrap(); let mut packet = vec![1]; packet.extend_from_slice(r.expose_to_transport()); packet }
        _ => unreachable!(),
    }
}

async fn relay_owned(front: TcpListener, ports: Vec<u16>, cancelled_sdk: bool, no_more_dials: oneshot::Receiver<()>) {
    let mut joins = Vec::new();
    for (index, target) in ports.iter().copied().enumerate() {
        let (mut inbound, _) = front.accept().await.unwrap();
        let mut outbound = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, target)).await.unwrap();
        joins.push(tokio::spawn(Box::pin(async move { bounded(async move {
            match tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await {
                Ok((up, down)) => assert!(up > 0 && down > 0, "both directions carried actual original IO"),
                Err(error) => assert!(cancelled_sdk && index == 1 && matches!(error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionAborted),
                    "only original cancelled SDK relay may terminate with close/reset: {error}"),
            }
        }).await })));
    }
    if cancelled_sdk {
        // Existing sixth-auxdial refusal fixture, not an SDK retry or delay.
        assert!(tokio::time::timeout(Duration::from_millis(35), front.accept()).await.is_err(),
            "unknown same account cannot make a sixth auxiliary dial");
    }
    tokio::select! { biased;
        dial = front.accept() => panic!("unexpected additional original/auxiliary dial: {dial:?}"),
        finished = no_more_dials => finished.unwrap(),
    }
    for join in joins { bounded(join).await.unwrap(); }
}
async fn listener() -> TcpListener { TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap() }
fn clone_acceptors(listener: TcpListener) -> (TcpListener, TcpListener) {
    // Clone before CommonPeer consumes either original listener.
    let original = listener.into_std().unwrap();
    let second = TcpListener::from_std(original.try_clone().unwrap()).unwrap();
    (TcpListener::from_std(original).unwrap(), second)
}

#[derive(Clone, Copy)]
enum LookupCase { Accepted, Negative(u8), Pending }
async fn three_peers(case: LookupCase) {
    let cert = Certificates::generate("IP:127.0.0.1");
    let front = listener().await; let port = front.local_addr().unwrap().port();
    let boot = listener().await; let sdk_listener = listener().await; let aux = listener().await;
    let ports = vec![boot.local_addr().unwrap().port(), sdk_listener.local_addr().unwrap().port(), aux.local_addr().unwrap().port()];
    let server = cert.server_config(&TLS13); let source_server = server.clone(); let sdk_server = server.clone();
    let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
    let account = bounded(manager.create(send_configuration(&cert.root, port, false), 100)).await.unwrap().account;
    let line = account.line_id.clone(); let db = temp.database.clone(); let sdk_line = line.clone();
    let aux_db = temp.database.clone(); let aux_line = line.clone();
    let (finished, finish) = oneshot::channel(); let (guard_done, guarded) = oneshot::channel();
    let (aux_observed, aux_seen) = oneshot::channel(); let (release_aux, auxiliary_ready) = oneshot::channel();
    let relay = tokio::spawn(Box::pin(async move { bounded(relay_owned(front, ports, false, guarded)).await }));
    let source = tokio::spawn(Box::pin(async move { bounded(bootstrap_peer(boot, source_server)).await }));
    let sdk = tokio::spawn(Box::pin(async move { bounded(async move {
        let mut peer = CommonPeer::open(sdk_listener, sdk_server).await;
        let own_uri = login_capture(&mut peer, SELECTOR).await;
        if !matches!(case, LookupCase::Negative(_)) {
            let submitted = send_call(&mut peer, &own_uri, &db, &sdk_line).await;
            peer.send(&[accepted_send_reply(submitted.request.unwrap())]).await;
        }
        idle_until_finish(&mut peer, SELECTOR, finish).await;
        finish_sdk(peer, &own_uri, SELECTOR).await;
    }).await }));
    let resolver = tokio::spawn(Box::pin(async move { bounded(async move {
        let mut peer = CommonPeer::open(aux, server).await;
        let id = auxiliary_call(&peer.next().await.unwrap());
        let row = send_row(&aux_db, &aux_line); marker(&row, SELECTOR); no_send_permit(&row, 0);
        aux_observed.send(row).unwrap(); auxiliary_ready.await.unwrap();
        peer.send(&[match case { LookupCase::Negative(v) => negative_auxiliary_reply(id, v), _ => reply(id, &aid(0)) }]).await;
        peer.no_more_common().await;
    }).await }));
    bounded(manager.start(&line, account.revision, 100)).await.unwrap(); wait_phase(&manager, &line, "hosted").await;
    healthy(&manager, &line).await; no_other_business(&manager, &temp.database, &line);
    assert_eq!(count_rows(&temp.database, "app_voice_runtime_leases", &line), 1);
    assert!(manager.im_send_history(&line, 128).unwrap().is_empty());
    if matches!(case, LookupCase::Accepted) {
        for (consent, write_consent) in [(false, false), (true, false), (false, true)] {
            assert_eq!(bounded(manager.send_im_message(&line, 1, ALIAS, text(), NONCE, consent, write_consent)).await.err(),
                Some("app_account_im_send_consent_required"));
        }
        assert_eq!(bounded(manager.send_im_message(&line, 2, ALIAS, text(), NONCE, true, true)).await.err(), Some("app_account_revision_conflict"));
        assert_eq!(count_rows(&temp.database, "app_voice_im_outbox", &line), 0);
    }
    let mut sending = Box::pin(manager.send_im_message(&line, 1, ALIAS, text(), NONCE, true, true));
    let pending = tokio::select! {
        result = sending.as_mut() => panic!("actual auxiliary has not replied: {result:?}"),
        row = bounded(aux_seen) => row.unwrap(),
    };
    no_send_permit(&pending, 0); assert_eq!(send_row(&temp.database, &line), pending);
    if matches!(case, LookupCase::Pending) {
        // Keep the original send future polled throughout concurrent admissions;
        // exact same nonce observes unknown DATA, not another auxiliary grant.
        tokio::select! {
            result = sending.as_mut() => panic!("purpose READ deliberately pending: {result:?}"),
            _ = bounded(async {
                let same = bounded(manager.send_im_message(&line, 1, ALIAS, text(), NONCE, true, true)).await.unwrap();
                public_view(&same, pending.id, "uncertain", false);
                assert_eq!(bounded(manager.send_im_message(&line, 1, ALIAS, changed_text(), NONCE, true, true)).await.err(), Some("app_account_im_send_nonce_conflict"));
                assert_eq!(bounded(manager.send_im_message(&line, 1, OTHER_ALIAS, text(), NONCE, true, true)).await.err(), Some("app_account_im_send_nonce_conflict"));
                for (recipient, nonce) in [(ALIAS, NEXT_NONCE), (OTHER_ALIAS, THIRD_NONCE), (UID_URI, FOURTH_NONCE)] {
                    assert_eq!(bounded(manager.send_im_message(&line, 1, recipient, text(), nonce, true, true)).await.err(), Some("app_account_im_send_uncertain"));
                }
                assert_eq!(send_row(&temp.database, &line), pending);
                assert_eq!(count_rows(&temp.database, "app_voice_im_outbox", &line), 1);
                let history = manager.im_send_history(&line, 128).unwrap(); assert_eq!(history.len(), 1);
                public_view(&history[0], pending.id, "uncertain", false);
                no_other_business(&manager, &temp.database, &line); healthy(&manager, &line).await;
            }) => {}
        }
    }
    release_aux.send(()).unwrap();
    let result = bounded(sending).await;
    let negative = matches!(case, LookupCase::Negative(_));
    if negative { assert_eq!(result.err(), Some("app_account_im_send_recipient_unconfirmed")); }
    else { public_view(&result.unwrap(), pending.id, "confirmed", true); }
    let terminal = send_row(&temp.database, &line); marker(&terminal, SELECTOR);
    assert_eq!(terminal.id, pending.id); assert_eq!(terminal.run, pending.run); assert_eq!(terminal.epoch, pending.epoch);
    if negative { no_send_permit(&terminal, 3); }
    else {
        assert_eq!((terminal.state, terminal.code, terminal.index, terminal.time), (1, Some(0), Some(INDEX), Some(ORIGINAL_TIME)));
        assert!(terminal.request.unwrap() > 0); assert_eq!(terminal.box_id.as_deref(), Some(b"P2P/1_2".as_slice()));
        assert_eq!(terminal.arguments_hash.as_ref().unwrap().len(), 32);
    }
    let same = bounded(manager.send_im_message(&line, 1, ALIAS, text(), NONCE, true, true)).await.unwrap();
    public_view(&same, pending.id, if negative { "not_sent" } else { "confirmed" }, !negative);
    assert_eq!(bounded(manager.send_im_message(&line, 1, OTHER_ALIAS, text(), NONCE, true, true)).await.err(), Some("app_account_im_send_nonce_conflict"));
    assert_eq!(send_row(&temp.database, &line), terminal);
    let history = manager.im_send_history(&line, 128).unwrap(); assert_eq!(history.len(), 1);
    public_view(&history[0], pending.id, if negative { "not_sent" } else { "confirmed" }, !negative);
    healthy(&manager, &line).await; no_other_business(&manager, &temp.database, &line);
    finished.send(()).unwrap();
    assert!(!bounded(manager.stop(&line, 1, 100)).await.unwrap().io_cleanup_required);
    lease_zero(&temp.database, &line); assert_eq!(bounded(manager.shutdown_all()).await, 0);
    bounded(source).await.unwrap(); bounded(sdk).await.unwrap(); bounded(resolver).await.unwrap();
    guard_done.send(()).unwrap(); bounded(relay).await.unwrap();
    assert_eq!(send_row(&temp.database, &line), terminal); no_other_business(&manager, &temp.database, &line);
}

#[tokio::test]
async fn send_recipient_actual_alias_purpose_uid_original_own_login_uri_text_statistics_two_i64_sql_and_all_joins() {
    Box::pin(bounded(three_peers(LookupCase::Accepted))).await;
}
#[tokio::test]
async fn send_recipient_actual_negative_queries_not_sent_no_send_id_or_private_grant_healthy_sdk_and_all_joins() {
    Box::pin(bounded(async {
        // Independent explicit fixtures, not retries of any original SDK/query.
        for variant in 0..3 { Box::pin(three_peers(LookupCase::Negative(variant))).await; }
    })).await;
}
#[tokio::test]
async fn send_recipient_actual_pending_query_same_nonce_unknown_metadata_conflicts_fresh_nonce_fence_release_once_and_all_joins() {
    Box::pin(bounded(three_peers(LookupCase::Pending))).await;
}

fn higher_status_update() -> Vec<u8> {
    let mut body = Writer::new(); body.properties(&[
        RpcProperty { key: n::im_status::LATEST, value: b"4294967300" },
        RpcProperty { key: n::im_status::LAST_READ, value: b"4294967300" },
        RpcProperty { key: n::im_status::LAST_RECEIVED, value: b"4294967300" },
        RpcProperty { key: n::im_status::PEER_READ, value: b"4294967300" },
    ]).unwrap();
    let mut a = Writer::versioned().unwrap(); a.bytes(n::im_status::STATUS_KIND).unwrap();
    a.properties(&[RpcProperty { key: b"Group.GroupId", value: b"1_2" }, RpcProperty { key: b"Group.UpdateTime", value: b"2" }]).unwrap();
    a.bytes(body.finish().expose_to_transport()).unwrap();
    let rpc = encode_request(91, &[RpcProperty { key: b"id", value: b"MessageReceiver" }, RpcProperty { key: b"__magic", value: &[9, 1] }],
        n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), a.finish().expose_to_transport()).unwrap();
    let mut packet = vec![0]; packet.extend_from_slice(rpc.expose_to_transport()); packet
}
async fn status_ack(peer: &mut CommonPeer) {
    loop {
        let packet = peer.next().await.unwrap();
        if packet.first() == Some(&1) {
            let r = decode_reply(&packet[1..]).unwrap(); assert_eq!(r.call_id, 91);
            assert_eq!(r.result_code, 0); assert_eq!(r.result_stream, &[0, 1, 0, 0, 0, 0]); return;
        }
        active_idle(peer, &packet, CHANGED_SELECTOR).await;
    }
}
fn committed_higher_status(db: &Database, line: &str) {
    assert_eq!(count_rows(db, "app_voice_im_status_properties", line), 4);
    for key in [n::im_status::LATEST, n::im_status::LAST_READ, n::im_status::LAST_RECEIVED, n::im_status::PEER_READ] {
        let value: (i64, Vec<u8>) = db.with_conn(|c| c.query_row(
            "SELECT update_time,status_value FROM app_voice_im_status_properties WHERE line_id=?1 AND status_key=?2",
            rusqlite::params![line, key], |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|_| "actual higher Status.Update commit")).unwrap();
        assert_eq!(value, (2, b"4294967300".to_vec()));
    }
}

#[tokio::test]
async fn send_recipient_actual_submitted_cancel_true_eof_stop_lease_zero_changed_selector_same_uid_restart_status_not_ack_and_all_joins() {
    Box::pin(bounded(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let front = listener().await; let port = front.local_addr().unwrap().port();
        let boot = listener().await; let sdk_listener = listener().await; let aux = listener().await;
        let boot_port = boot.local_addr().unwrap().port(); let sdk_port = sdk_listener.local_addr().unwrap().port();
        let aux_port = aux.local_addr().unwrap().port();
        let (boot1, boot2) = clone_acceptors(boot); let (sdk1, sdk2) = clone_acceptors(sdk_listener);
        let server = cert.server_config(&TLS13); let source1_server = server.clone(); let source2_server = server.clone();
        let first_server = server.clone(); let second_server = server.clone();
        let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
        let account = bounded(manager.create(send_configuration(&cert.root, port, false), 100)).await.unwrap().account;
        let line = account.line_id.clone(); let first_line = line.clone(); let second_line = line.clone();
        let first_db = temp.database.clone(); let second_db = temp.database.clone();
        let aux_db = temp.database.clone(); let aux_line = line.clone();
        let (sent, submitted) = oneshot::channel(); let (cancel, cancelled) = oneshot::channel();
        let (closed, original_closed) = oneshot::channel(); let (ready, second_ready) = oneshot::channel();
        let (finished, finish) = oneshot::channel(); let (guard_done, guarded) = oneshot::channel();
        let (source_start, source_starting) = oneshot::channel(); let (sdk_start, sdk_starting) = oneshot::channel();
        let relay = tokio::spawn(Box::pin(async move {
            bounded(relay_owned(front, vec![boot_port, sdk_port, aux_port, boot_port, sdk_port], true, guarded)).await;
        }));
        let source1 = tokio::spawn(Box::pin(async move { bounded(bootstrap_peer(boot1, source1_server)).await }));
        let source2 = tokio::spawn(Box::pin(async move { bounded(async move { source_starting.await.unwrap(); bootstrap_peer(boot2, source2_server).await; }).await }));
        let first = tokio::spawn(Box::pin(async move { bounded(async move {
            let mut peer = CommonPeer::open(sdk1, first_server).await;
            let own_uri = login_capture(&mut peer, SELECTOR).await;
            let row = send_call(&mut peer, &own_uri, &first_db, &first_line).await;
            let id = row.id; sent.send(row).unwrap(); cancelled.await.unwrap();
            retired_input_eof(&mut peer).await; drop(peer);
            assert_eq!(send_row(&first_db, &first_line).id, id);
            closed.send(()).unwrap();
        }).await }));
        let resolver = tokio::spawn(Box::pin(async move { bounded(async move {
            let mut peer = CommonPeer::open(aux, server).await;
            let id = auxiliary_call(&peer.next().await.unwrap());
            let row = send_row(&aux_db, &aux_line); marker(&row, SELECTOR); no_send_permit(&row, 0);
            peer.send(&[reply(id, &aid(0))]).await; peer.no_more_common().await;
        }).await }));
        let second = tokio::spawn(Box::pin(async move { bounded(async move {
            sdk_starting.await.unwrap();
            let mut peer = CommonPeer::open(sdk2, second_server).await;
            // SAME original returned UID/token/mainAccount, CHANGED selector.
            let own_uri = login_capture(&mut peer, CHANGED_SELECTOR).await;
            peer.send(&[higher_status_update()]).await; status_ack(&mut peer).await;
            committed_higher_status(&second_db, &second_line); ready.send(()).unwrap();
            idle_until_finish(&mut peer, CHANGED_SELECTOR, finish).await;
            finish_sdk(peer, &own_uri, CHANGED_SELECTOR).await;
        }).await }));
        bounded(manager.start(&line, 1, 100)).await.unwrap(); wait_phase(&manager, &line, "hosted").await;
        healthy(&manager, &line).await; no_other_business(&manager, &temp.database, &line);
        let mut sending = Box::pin(manager.send_im_message(&line, 1, ALIAS, text(), NONCE, true, true));
        let original = tokio::select! {
            result = sending.as_mut() => panic!("actual full SEND has no private result: {result:?}"),
            row = bounded(submitted) => row.unwrap(),
        };
        // Main supplies this cfg(test) original GetImBinding completion. It
        // allocates no ID/wire and causally follows send_packets/submitted and
        // private pending insertion. The SEND future stays polled until then.
        tokio::select! { biased;
            result = sending.as_mut() => panic!("no original WRITE result before barrier: {result:?}"),
            result = bounded(manager.im_send_actor_barrier_for_test(&line, 1)) => result.unwrap(),
        }
        assert_eq!(send_row(&temp.database, &line), original);
        drop(sending); cancel.send(()).unwrap();
        wait_phase(&manager, &line, "disconnected").await;
        let unknown = manager.im_send_history(&line, 128).unwrap(); assert_eq!(unknown.len(), 1);
        public_view(&unknown[0], original.id, "uncertain", false);
        assert!(!bounded(manager.stop(&line, 1, 100)).await.unwrap().io_cleanup_required);
        bounded(original_closed).await.unwrap(); lease_zero(&temp.database, &line);
        bounded(source1).await.unwrap(); bounded(resolver).await.unwrap(); bounded(first).await.unwrap();
        assert_eq!(send_row(&temp.database, &line), original, "true full WRITE unknown retained verbatim");
        assert_eq!(bounded(manager.shutdown_all()).await, 0); drop(manager);

        let fresh = AccountManager::new(temp.database.clone());
        let replacement = bounded(fresh.replace(&line, 1, send_configuration(&cert.root, port, true), 100)).await.unwrap();
        assert_eq!(replacement.account.revision, 2);
        assert_eq!(send_row(&temp.database, &line), original, "selector/revision replacement does not erase unknown");
        source_start.send(()).unwrap(); sdk_start.send(()).unwrap();
        bounded(fresh.start(&line, 2, 100)).await.unwrap(); wait_phase(&fresh, &line, "hosted").await;
        bounded(second_ready).await.unwrap(); healthy(&fresh, &line).await;
        committed_higher_status(&temp.database, &line);
        assert_ne!(original.identity, private_digest(&[b"jegotrip-native-im-context-v1\0", b"fixture-user", CHANGED_SELECTOR]));
        assert_eq!(bounded(fresh.send_im_message(&line, 2, ALIAS, text(), NONCE, true, true)).await.err(), Some("app_account_im_send_nonce_conflict"));
        for (recipient, nonce) in [(ALIAS, NEXT_NONCE), (UID_URI, THIRD_NONCE), ("1-2", FOURTH_NONCE)] {
            assert_eq!(bounded(fresh.send_im_message(&line, 2, recipient, text(), nonce, true, true)).await.err(), Some("app_account_im_send_uncertain"));
        }
        assert_eq!(send_row(&temp.database, &line), original, "higher Status.Update is not an original WRITE ACK");
        assert_eq!(count_rows(&temp.database, "app_voice_im_outbox", &line), 1);
        let history = fresh.im_send_history(&line, 128).unwrap(); assert_eq!(history.len(), 1);
        public_view(&history[0], original.id, "uncertain", false);
        no_other_business(&fresh, &temp.database, &line); healthy(&fresh, &line).await;
        assert_eq!(count_rows(&temp.database, "app_voice_runtime_leases", &line), 1);
        finished.send(()).unwrap(); assert!(!bounded(fresh.stop(&line, 2, 100)).await.unwrap().io_cleanup_required);
        lease_zero(&temp.database, &line); assert_eq!(bounded(fresh.shutdown_all()).await, 0);
        bounded(source2).await.unwrap(); bounded(second).await.unwrap();
        guard_done.send(()).unwrap(); bounded(relay).await.unwrap();
        assert_eq!(send_row(&temp.database, &line), original);
        public_view(&fresh.im_send_history(&line, 128).unwrap()[0], original.id, "uncertain", false);
        no_other_business(&fresh, &temp.database, &line);
    })).await;
}
