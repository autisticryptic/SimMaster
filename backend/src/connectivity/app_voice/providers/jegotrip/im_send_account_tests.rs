//! Actions-only actual AccountManager -> original Router/TLS/Common SDK actor
//! -> userSend WRITE -> SQL result/uncertainty -> stop and physical peer Joins.
//! Mounted only as im_received_account_tests::sends. No SDK library, device,
//! Provider API or real outbound business service is used by these fixtures.
#[path="im_send_option_account_tests.rs"]
mod options;
#[path="im_send_account_hardening_tests.rs"]
mod hardening;
use super::*;
use n::{im_inbox::{ImSendMessage, ImSendView}, im_send_policy::NativeImSendConfig};

const OWN_URI: &[u8] = b"[username:fixture-account@fixture.invalid]";
const MAIN_ACCOUNT: &[u8] = b"fixture-returned-main-account";
const NONCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEXT_NONCE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TEXT: &str = "private \"original\"\nIM 🥘";
const TEXT_JSON: &str = "{\"text\":\"private \\\"original\\\"\\nIM 🥘\"}";
const INFO: &str = "private original Info";
const INDEX: i64 = 4_294_967_297;
const TIME: i64 = -8_589_934_593;

fn configuration(root: &str, port: u16, enabled: bool) -> AccountConfig {
    let mut cfg = config(root, port, false);
    // R11 requires native current authentication/IM installation, NOT a
    // receive-sync installation, received receipt or user-read WRITE grant.
    cfg.profile.im_sync = None;
    cfg.profile.im_send = enabled.then_some(NativeImSendConfig {
        send_text: true, send_info: true, auth_country_code: None,
    });
    assert!(cfg.profile.im_receipts.is_none());
    assert!(cfg.profile.im_user_read.is_none());
    assert!(cfg.profile.im_read_status.is_none());
    assert!(cfg.profile.incoming.is_none() && cfg.profile.client_ex);
    assert_eq!(cfg.credentials.sdk.account_identity.expose_to_backend(), "fixture-account");
    cfg
}

fn body(group: bool) -> ImSendMessage {
    if group {
        ImSendMessage::components(
            Some("Fixture.Subtype"), INFO,
            &[(b"../never-read".to_vec(), vec![0, 255, 128, 13, 10])],
            &[(b"opaque.parameter".to_vec(), b"not-interpreted-phone-token".to_vec())],
            None,
        ).unwrap()
    } else {
        ImSendMessage::text(TEXT).unwrap()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct OutboxRow {
    id: i64,
    state: i64,
    request: Option<i32>,
    code: Option<i64>,
    index: Option<i64>,
    time: Option<i64>,
    box_id: Option<Vec<u8>>,
    revision: i64,
    run: String,
    epoch: String,
    arguments_hash: Option<Vec<u8>>,
}
fn outbox(db: &crate::platform::db::Database, line: &str) -> OutboxRow {
    db.with_conn(|c| c.query_row(
        "SELECT id,state,request_id,result_code,message_index,original_time,box_id,revision,run_nonce,scope_epoch,arguments_hash FROM app_voice_im_outbox WHERE line_id=?1",
        [line], |r| Ok(OutboxRow {
            id: r.get(0)?, state: r.get(1)?, request: r.get(2)?, code: r.get(3)?,
            index: r.get(4)?, time: r.get(5)?, box_id: r.get(6)?, revision: r.get(7)?,
            run: r.get(8)?, epoch: r.get(9)?, arguments_hash: r.get(10)?,
        }),
    ).map_err(|_| "fixture outbox")).unwrap()
}
fn rows(db: &crate::platform::db::Database, table: &str, line: &str) -> i64 {
    db.with_conn(|c| c.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE line_id=?1"), [line], |r| r.get(0),
    ).map_err(|_| "fixture row count")).unwrap()
}
fn no_other_business(manager: &AccountManager, db: &crate::platform::db::Database, line: &str) {
    for table in [
        "app_voice_im_messages", "app_voice_im_receive_cursors", "app_voice_im_read_receipts",
        "app_voice_im_user_reads", "app_voice_im_read_status_actions", "app_voice_im_read_status_targets",
    ] {
        assert_eq!(rows(db, table, line), 0, "SEND never installs or manufactures {table}");
    }
    assert!(manager.im_messages(line, 16).unwrap().is_empty());
    assert!(manager.messages(line, 16).unwrap().is_empty());
    assert!(db.get_call_history_for_line(line, 16, 0).unwrap().is_empty());
    assert!(db.get_recent_app_events(Some(line), 16).unwrap().iter().all(|e| e.event_type != "im.received"));
}
fn released(db: &crate::platform::db::Database, line: &str) {
    assert_eq!(rows(db, "app_voice_runtime_leases", line), 0, "true stop releases the original exclusive lease");
    assert!(!db.with_conn(|c| n::account_runs::ownership_pending(c, line)).unwrap());
}
async fn phase(manager: &AccountManager, line: &str, expected: &str) {
    bounded(async {
        loop {
            if manager.get(line, 100).await.unwrap().phase == expected { break; }
            tokio::task::yield_now().await;
        }
    }).await;
}

// This positive case independently inspects the current SDK login's OWN URI
// and returns a deliberately different mainAccount. Neither returned DATA
// value may substitute for the account parameter in the original userSend.
async fn login_with_distinct_main_account(peer: &mut CommonPeer) {
    let packet = peer.next().await.unwrap();
    assert_eq!(packet[0], 0);
    let login = decode_request(&packet[1..]).unwrap();
    assert_eq!(login.method, n::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
    assert_eq!(login.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"AccountEntry");
    assert_eq!(login.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-original-pwd");
    assert!(!login.metadata.iter().any(|p| p.key == b"token"));
    let mut args = Reader::versioned(login.arguments).unwrap();
    assert_eq!(args.bytes().unwrap(), OWN_URI);
    assert_eq!(args.bytes().unwrap(), b"fixture-original-selector");
    let properties = args.properties().unwrap();
    assert_eq!(properties.iter().find(|p| p.key == b"__app").unwrap().value, b"fixture-app");
    assert_eq!(properties.iter().find(|p| p.key == b"__domain").unwrap().value, b"fixture-domain");
    args.finish().unwrap();
    let mut result = Writer::new();
    result.boolean(true).unwrap();
    result.bytes(b"fixture-user").unwrap();
    result.bytes(b"fixture-auth").unwrap();
    result.properties(&[RpcProperty { key: b"mainAccount", value: MAIN_ACCOUNT }]).unwrap();
    result.bytes(b"").unwrap();
    peer.send(&[reply(login.call_id, result.finish().expose_to_transport())]).await;
    let (_, flag) = hosting_call(peer).await;
    assert_eq!(flag, b"4");
}

async fn send_call(peer: &mut CommonPeer, group: bool) -> i32 {
    // The parent's query helper checks the original direct RPC's #Group id
    // and current token; its idle swap handling rejects nested business work.
    let packet = query(peer, n::im_send_data::USER_SEND_METHOD).await;
    assert_eq!(packet[0], 0, "not a nested swap request or a received ACK");
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.method, b"userSend.Message.Message");
    assert_eq!(&request.arguments[..5], &[0, 0, 1, 0, 0]);
    let mut a = Reader::versioned(request.arguments).unwrap();
    assert_eq!(a.bytes().unwrap(), if group { b"Org/1-2".as_slice() } else { b"P2P/1_2".as_slice() });
    assert_eq!(a.bytes().unwrap(), if group { b"Info.Fixture.Subtype".as_slice() } else { b"Text".as_slice() });
    assert_eq!(a.bytes().unwrap(), if group { br#"{"content":"private original Info"}"#.as_slice() } else { TEXT_JSON.as_bytes() });
    let attachments = a.properties().unwrap();
    if group {
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].key, b"../never-read");
        assert_eq!(attachments[0].value, &[0, 255, 128, 13, 10], "opaque bytes, never a file read");
    } else { assert!(attachments.is_empty()); }
    let parameters = a.properties().unwrap();
    assert_eq!(parameters.len(), if group { 2 } else { 1 });
    let account = parameters.iter().find(|p| p.key == b"account").unwrap().value;
    assert_eq!(account, OWN_URI);
    assert_ne!(account, b"fixture-user");
    assert_ne!(account, MAIN_ACCOUNT);
    assert_ne!(account, b"fixture-account");
    if group {
        assert_eq!(parameters.iter().find(|p| p.key == b"opaque.parameter").unwrap().value, b"not-interpreted-phone-token");
    }
    let notify = a.properties().unwrap();
    assert_eq!(notify.len(), if group { 3 } else { 4 });
    for (key, value) in [
        (b"Statistics.AppId".as_slice(), b"fixture-app".as_slice()),
        (b"Statistics.DomainId".as_slice(), b"fixture-domain".as_slice()),
        (b"Statistics.CountryCode".as_slice(), b"".as_slice()),
    ] {
        assert_eq!(notify.iter().find(|p| p.key == key).unwrap().value, value);
    }
    assert!(!notify.iter().any(|p| p.key == b"Notify.CountryCode"));
    if group { assert!(!notify.iter().any(|p| p.key == b"Notify.Text")); }
    else { assert_eq!(notify.iter().find(|p| p.key == b"Notify.Text").unwrap().value, TEXT.as_bytes()); }
    a.finish().unwrap();
    request.call_id
}
fn send_reply(id: i32, yes: bool, code: u32) -> Vec<u8> {
    // Success/false is bool + TWO signed64 + Common footer. Original class1
    // DC5638 instead consumes a BE16 supported-method-version list; it cannot
    // be represented by fabricating a normal SEND result or automatic retry.
    let mut w = Writer::new();
    if code >> 16 == 1 { w.u16(1).unwrap(); w.u16(1).unwrap(); }
    else { w.boolean(yes).unwrap(); w.i64(INDEX).unwrap(); w.i64(TIME).unwrap(); w.bytes(b"").unwrap(); }
    let rpc = encode_reply(id, code, w.finish().expose_to_transport()).unwrap();
    let mut packet = vec![1]; packet.extend_from_slice(rpc.expose_to_transport()); packet
}
fn public_result(view: &ImSendView, kind: &str, state: &str, code: u32) {
    assert!(view.id > 0);
    assert_eq!(view.kind, kind); assert_eq!(view.state, state);
    assert_eq!(view.message_index.as_deref(), if code == 0 { Some("4294967297") } else { None });
    assert_eq!(view.original_time.as_deref(), if code == 0 { Some("-8589934593") } else { None });
    assert_eq!(view.nonce_tag.len(), 64);
    let public = serde_json::to_string(view).unwrap();
    for private in [NONCE, TEXT, INFO, "fixture", "Org/1-2", "P2P/1_2", "user_nonce", "identity_key", "scope_epoch", "arguments_hash", "never-read", "phone-token", "Statistics."] {
        assert!(!public.contains(private), "SEND history exposes only sanitized result DATA");
    }
}

async fn replied(group: bool, yes: bool, code: u32, check_consents: bool) {
    let cert = Certificates::generate("IP:127.0.0.1");
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
    let account = manager.create(configuration(&cert.root, port, true), 100).await.unwrap().account;
    let line = account.line_id.clone(); let db = temp.database.clone(); let peer_line = line.clone();
    let (finish, finishing) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(Box::pin(async move { bounded(async move {
        let mut peer = CommonPeer::open(listener, server).await;
        if check_consents { login_with_distinct_main_account(&mut peer).await; }
        else { login_host(&mut peer).await; }
        let id = send_call(&mut peer, group).await;
        let row = outbox(&db, &peer_line);
        assert!(row.id > 0); assert_eq!((row.state, row.request, row.code, row.index, row.time), (0, Some(id), None, None, None));
        assert_eq!(row.box_id.as_deref(), Some(if group { b"Org/1-2".as_slice() } else { b"P2P/1_2".as_slice() }));
        assert_eq!(row.revision, 1); assert_eq!(row.run.len(), 32); assert!(!row.epoch.is_empty());
        assert_eq!(row.arguments_hash.as_ref().unwrap().len(), 32);
        peer.send(&[send_reply(id, yes, code)]).await;
        // Every same-nonce API call completes while this original peer stays
        // live. Then normal unhosting rejects any replay/direct business RPC.
        finishing.await.unwrap(); finish_account(&mut peer).await;
    }).await }));
    bounded(manager.start(&line, account.revision, 100)).await.unwrap();
    phase(&manager, &line, "hosted").await;
    assert!(manager.im_send_history(&line, 128).unwrap().is_empty(), "installation and SDK login are not send consent");
    let recipient = if group { "1-2" } else { "1_2" };
    if check_consents {
        for (consent, write_consent) in [(false, false), (false, true), (true, false)] {
            assert_eq!(bounded(manager.send_im_message(&line, account.revision, recipient, body(group), NONCE, consent, write_consent)).await.err(), Some("app_account_im_send_consent_required"));
        }
        assert_eq!(bounded(manager.send_im_message(&line, account.revision + 1, recipient, body(group), NONCE, true, true)).await.err(), Some("app_account_revision_conflict"));
        assert_eq!(rows(&temp.database, "app_voice_im_outbox", &line), 0);
    }
    let result = bounded(manager.send_im_message(&line, account.revision, recipient, body(group), NONCE, true, true)).await.unwrap();
    let state = if yes && code == 0 { "confirmed" } else { "rejected" };
    public_result(&result, if group { "info" } else { "text" }, state, code);
    let row = outbox(&temp.database, &line);
    assert_eq!(row.id, result.id); assert_eq!(row.state, if state == "confirmed" { 1 } else { 2 });
    assert_eq!(row.code, Some(i64::from(code)));
    assert_eq!(row.index, if code == 0 { Some(INDEX) } else { None });
    assert_eq!(row.time, if code == 0 { Some(TIME) } else { None });
    for _ in 0..2 {
        let replay = bounded(manager.send_im_message(&line, account.revision, recipient, body(group), NONCE, true, true)).await.unwrap();
        assert_eq!(replay.id, result.id); assert_eq!(replay.nonce_tag, result.nonce_tag);
        public_result(&replay, result.kind, state, code);
        assert_eq!(outbox(&temp.database, &line), row, "history replay allocates no new original RPC");
    }
    let history = manager.im_send_history(&line, 128).unwrap();
    assert_eq!(history.len(), 1); assert_eq!(history[0].id, result.id); assert_eq!(history[0].state, state);
    no_other_business(&manager, &temp.database, &line);
    let current = manager.get(&line, 100).await.unwrap();
    assert_eq!(current.phase, "hosted"); assert!(!current.incoming_call_ready && !current.outbound_authorized);
    finish.send(()).unwrap();
    assert!(!bounded(manager.stop(&line, account.revision, 100)).await.unwrap().io_cleanup_required);
    bounded(serving).await.unwrap(); released(&temp.database, &line);
    assert_eq!(outbox(&temp.database, &line), row, "true stop retains explicit send history");
}

#[tokio::test]
async fn send_account_two_explicit_consents_revision_fence_and_original_direct_uid_exact_own_uri_text_statistics() {
    Box::pin(replied(false, true, 0, true)).await;
}
#[tokio::test]
async fn send_account_original_group_info_byte_attachments_opaque_parameters_and_matched_two_i64_result() {
    Box::pin(replied(true, true, 0, false)).await;
}
#[tokio::test]
async fn send_account_original_false_is_known_rejected_and_same_nonce_only_reads_history_without_rpc() {
    Box::pin(replied(false, false, 0, false)).await;
}
#[tokio::test]
async fn send_account_original_version_error_is_known_rejected_and_same_nonce_never_retries_rpc() {
    Box::pin(replied(false, true, 65537, false)).await;
}
#[tokio::test]
async fn send_account_absent_policy_has_no_write_intent_or_rpc_even_with_both_consents() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
        let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
        let account = manager.create(configuration(&cert.root, port, false), 100).await.unwrap().account;
        let line = account.line_id.clone(); let (finish, finishing) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(Box::pin(async move { bounded(async move {
            let mut peer = CommonPeer::open(listener, server).await; login_host(&mut peer).await;
            finishing.await.unwrap(); finish_account(&mut peer).await;
        }).await }));
        bounded(manager.start(&line, account.revision, 100)).await.unwrap(); phase(&manager, &line, "hosted").await;
        for recipient in ["1_2", "1-2"] {
            assert_eq!(bounded(manager.send_im_message(&line, account.revision, recipient, body(recipient == "1-2"), NONCE, true, true)).await.err(), Some("app_account_im_send_not_installed"));
        }
        assert!(manager.im_send_history(&line, 128).unwrap().is_empty());
        assert_eq!(rows(&temp.database, "app_voice_im_outbox", &line), 0);
        no_other_business(&manager, &temp.database, &line);
        finish.send(()).unwrap();
        assert!(!bounded(manager.stop(&line, account.revision, 100)).await.unwrap().io_cleanup_required);
        bounded(serving).await.unwrap(); released(&temp.database, &line);
    }).await;
}

fn retired_idle(packet: &[u8]) {
    if matches!(packet, [2] | [14]) { return; }
    assert_eq!(packet.first(), Some(&0), "no reply, release, partial control or arbitrary packet on retired SDK");
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(), "no SEND/replay/logout/direct business after full cancellation");
    assert_eq!(request.metadata.len(), 2);
    assert_eq!(request.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"AccountEntry");
    assert_eq!(request.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
    let mut a = Reader::versioned(request.arguments).unwrap();
    assert_eq!(a.bytes().unwrap(), b"fixture-user");
    assert_eq!(a.bytes().unwrap(), b"fixture-original-selector");
    let batch = n::rpc_batch::read_payload(&mut a).unwrap();
    assert!(batch.requests.is_empty() && batch.results.is_empty(), "unhosting/business/results are not an idle original poll");
    a.finish().unwrap();
}
async fn retired_eof(peer: &mut CommonPeer) {
    // INPUT ONLY: next() would emit Channel ACK on an already retired fd.
    for _ in 0..128 {
        match peer.next_input_only().await { Some(packet) => retired_idle(&packet), None => return }
    }
    panic!("retired original peer exceeded bounded record count");
}
fn higher_status(id: i32) -> Vec<u8> {
    // Genuine original ONLINE Status.Update (positive commit ACK even without
    // im_sync), not Notify.Command=Message which needs a received body.
    let mut body = Writer::new();
    body.properties(&[
        RpcProperty { key: n::im_status::LATEST, value: b"4294967300" },
        RpcProperty { key: n::im_status::LAST_READ, value: b"4294967300" },
        RpcProperty { key: n::im_status::LAST_RECEIVED, value: b"4294967300" },
        RpcProperty { key: n::im_status::PEER_READ, value: b"4294967300" },
    ]).unwrap();
    let mut args = Writer::versioned().unwrap(); args.bytes(n::im_status::STATUS_KIND).unwrap();
    args.properties(&[
        RpcProperty { key: b"Group.GroupId", value: b"1_2" },
        RpcProperty { key: b"Group.UpdateTime", value: b"2" },
    ]).unwrap(); args.bytes(body.finish().expose_to_transport()).unwrap();
    direct(id, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), args.finish().expose_to_transport(), b"MessageReceiver", None)
}

#[tokio::test]
async fn send_account_full_cancel_unknown_true_stop_fresh_manager_and_higher_status_cannot_enable_new_nonce_or_second_sdk_send() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
        let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
        let account = manager.create(configuration(&cert.root, port, true), 100).await.unwrap().account;
        let line = account.line_id.clone(); let db = temp.database.clone(); let peer_line = line.clone();
        let (sent, submitted) = tokio::sync::oneshot::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let (closed, first_closed) = tokio::sync::oneshot::channel();
        let (ready, second_ready) = tokio::sync::oneshot::channel();
        let (finish, finishing) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(Box::pin(async move { bounded(async move {
            let original = listener.into_std().unwrap();
            let second = TcpListener::from_std(original.try_clone().unwrap()).unwrap();
            let first = TcpListener::from_std(original).unwrap();
            let mut peer = CommonPeer::open(first, server.clone()).await; login_host(&mut peer).await;
            let id = send_call(&mut peer, false).await;
            let row = outbox(&db, &peer_line);
            assert_eq!((row.state, row.request, row.code), (0, Some(id), None));
            sent.send(row).unwrap(); cancelled.await.unwrap();
            retired_eof(&mut peer).await; drop(peer); closed.send(()).unwrap();
            let mut peer = CommonPeer::open(second, server).await; login_host(&mut peer).await;
            peer.send(&[higher_status(91)]).await;
            next_reply(&mut peer, 91, true).await;
            let status: (i64, Vec<u8>) = db.with_conn(|c| c.query_row(
                "SELECT update_time,status_value FROM app_voice_im_status_properties WHERE line_id=?1 AND status_key=?2",
                rusqlite::params![peer_line, n::im_status::LAST_READ], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(|_| "fixture higher status")).unwrap();
            assert_eq!(status, (2, b"4294967300".to_vec()));
            ready.send(()).unwrap(); finishing.await.unwrap();
            // Any SEND, refresh, received/user-read WRITE or replay on SDK #2
            // fails the original unhosting helper before normal logout/EOF.
            finish_account(&mut peer).await;
        }).await }));
        bounded(manager.start(&line, account.revision, 100)).await.unwrap(); phase(&manager, &line, "hosted").await;
        let mut sending = Box::pin(manager.send_im_message(&line, account.revision, "1_2", body(false), NONCE, true, true));
        let before = tokio::select! {
            result = sending.as_mut() => panic!("fully submitted original SEND has no reply: {result:?}"),
            row = bounded(submitted) => row.unwrap(),
        };
        tokio::select! {
            result = sending.as_mut() => panic!("original SEND result still absent: {result:?}"),
            result = bounded(manager.im_send_actor_barrier_for_test(&line, account.revision)) => result.unwrap(),
        }
        drop(sending); cancel.send(()).unwrap();
        phase(&manager, &line, "disconnected").await;
        assert_eq!(manager.im_send_history(&line, 128).unwrap()[0].state, "uncertain");
        assert!(!bounded(manager.stop(&line, account.revision, 100)).await.unwrap().io_cleanup_required);
        bounded(first_closed).await.unwrap(); released(&temp.database, &line);
        assert_eq!(outbox(&temp.database, &line), before, "possible original full WRITE stays unknown after true stop");
        drop(manager);
        let fresh = AccountManager::new(temp.database.clone());
        bounded(fresh.start(&line, account.revision, 100)).await.unwrap();
        phase(&fresh, &line, "hosted").await; bounded(second_ready).await.unwrap();
        let replay = bounded(fresh.send_im_message(&line, account.revision, "1_2", body(false), NONCE, true, true)).await.unwrap();
        assert_eq!(replay.id, before.id); assert_eq!(replay.state, "uncertain");
        assert_eq!(replay.message_index, None); assert_eq!(replay.original_time, None);
        assert_eq!(bounded(fresh.send_im_message(&line, account.revision, "1_2", body(false), NEXT_NONCE, true, true)).await.err(), Some("app_account_im_send_uncertain"));
        assert_eq!(outbox(&temp.database, &line), before);
        assert_eq!(fresh.im_send_history(&line, 128).unwrap().len(), 1);
        no_other_business(&fresh, &temp.database, &line);
        assert_eq!(fresh.get(&line, 100).await.unwrap().phase, "hosted");
        finish.send(()).unwrap();
        assert!(!bounded(fresh.stop(&line, account.revision, 100)).await.unwrap().io_cleanup_required);
        bounded(serving).await.unwrap(); released(&temp.database, &line);
        assert_eq!(outbox(&temp.database, &line), before);
        assert_eq!(fresh.im_send_history(&line, 128).unwrap()[0].state, "uncertain");
    }).await;
}
