//! Source-only original Info runtime/SQLite regressions for im_protocol_tests::info.
//! No SDK/device/network execution here; run only in the existing Actions test stack.
//! DATA selection never mints an SMS, call, SetReceived, user-read or send grant.
use super::*;
#[path="im_info_runtime_epoch_tests.rs"]
mod epochs;
#[path="im_info_runtime_cursor_tests.rs"]
mod cursors;
use n::{
    im_content::ImContent,
    im_inbox::{CursorSession, ImMessageView},
    im_info::ImInfo,
    im_text::{ImText, TextField},
    rpc_im::{ImQuery, OwnedImReply},
};

const INFO: &[u8] = br#"{"content":"visible info"}"#;
const PRIVATE_INFO: &[u8] = br#"{"content":"visible info","text":"private-text-alias","imdnId":"private-imdn","csnId":"private-csn","displayName":"private-display","userData":"private-user-data","OnlineMsg":"private-online","uid":"private-forged-uid","token":"private-forged-token","endpoint":"private-endpoint","grant":true}"#;
const COMMON_INFO: [(TextField, &str); 5] = [
    (TextField::ImdnId, "private-imdn"),
    (TextField::ConversationId, "private-csn"),
    (TextField::DisplayName, "private-display"),
    (TextField::UserData, "private-user-data"),
    (TextField::OnlineMessage, "private-online"),
];

fn info(f: &mut Fixture, rpc: i32, box_id: &[u8], index: i64, kind: &[u8], body: &[u8]) -> Box<IncomingImDelivery> {
    take(f.receive(rpc, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &more(box_id, index, kind, body), None))
}
fn views(f: &Fixture) -> Vec<ImMessageView> {
    f.temp.database.with_conn(|conn| n::im_inbox::list(conn, f.run.line_id(), 128)).unwrap()
}
fn event_count(f: &Fixture) -> usize {
    f.temp.database.get_recent_app_events(Some(f.run.line_id()), 128).unwrap()
        .iter().filter(|event| event.event_type == "im.received").count()
}
fn no_other_grants(f: &Fixture) {
    assert!(f.temp.database.get_sms_messages_for_channel(16, 0, None, Some(f.run.line_id())).unwrap().is_empty());
    assert!(f.temp.database.get_call_history_for_line(f.run.line_id(), 16, 0).unwrap().is_empty());
    let stored = f.temp.database.with_conn(|conn| account_store::load(conn, f.run.line_id())).unwrap().unwrap();
    let profile = &stored.config.profile;
    assert!(profile.im_receipts.is_none());
    assert!(profile.im_user_read.is_none());
    assert!(profile.im_read_status.is_none());
    assert!(profile.im_send.is_none());
    let writes: i64 = f.temp.database.with_conn(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM app_voice_im_read_receipts) +
                (SELECT COUNT(*) FROM app_voice_im_user_reads) +
                (SELECT COUNT(*) FROM app_voice_im_read_status_actions) +
                (SELECT COUNT(*) FROM app_voice_im_outbox)", [], |row| row.get(0)).map_err(|_| "fixture")).unwrap();
    assert_eq!(writes, 0);
}
fn private_projection(value: &str) {
    for private in [
        "private-text-alias", "private-imdn", "private-csn", "private-display",
        "private-user-data", "private-online", "private-forged-uid", "private-forged-token",
        "private-endpoint", "private-sender-uid", "private-native-account",
        "private-attachment", "identity_key", "fingerprint", "receipt_epoch",
        "box_id", "sender_uid", "parameters", "info_epoch", "\"body\"", "\"grant\"",
    ] {
        assert!(!value.contains(private), "private projection marker: {private}");
    }
}
// Local read helpers: im_protocol_tests::cursors's helpers are deliberately private.
fn refresh_result(target: i64) -> Vec<u8> {
    let target = target.to_string();
    let mut w = Writer::new();
    w.boolean(true).unwrap(); w.u32(1).unwrap(); w.bytes(b"1_2").unwrap();
    w.properties(&[
        RpcProperty { key: n::im_status::PEER_READ, value: b"0" },
        RpcProperty { key: n::im_status::LATEST, value: target.as_bytes() },
    ]).unwrap();
    w.i64(1).unwrap(); w.bytes(b"").unwrap();
    w.finish().expose_to_transport().to_vec()
}
fn recv_result(records: &[(i64, &[u8], &[u8])]) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(true).unwrap(); w.u32(records.len() as u32).unwrap();
    for (index, kind, content) in records {
        let message = n::im_data::ImMessage {
            index: *index, time: -123, sender_uid: b"private-sender-uid", kind, content,
            attachments: vec![RpcProperty { key: b"../private-attachment", value: b"private-attachment-bytes" }],
            parameters: vec![RpcProperty { key: b"account", value: b"private-native-account" }],
        };
        let raw = message.encode().unwrap();
        // The original recv stream embeds records, not a length-prefixed list of BLOBs.
        w.i64(message.index).unwrap(); w.i64(message.time).unwrap();
        w.bytes(message.sender_uid).unwrap(); w.bytes(message.kind).unwrap(); w.bytes(message.content).unwrap();
        w.properties(&message.attachments).unwrap(); w.properties(&message.parameters).unwrap();
        assert!(raw.expose_to_transport().len() >= 36);
    }
    w.bytes(b"").unwrap(); w.finish().expose_to_transport().to_vec()
}
fn read_reply(f: &mut Fixture, query: Arc<ImQuery>, bytes: &[u8]) -> OwnedImReply {
    let request = f.backend.prepare_im_read(query, f.gate.clone(), Instant::now() + Duration::from_secs(3)).unwrap();
    let events = f.backend.on_plaintext_packet(&f.owner, &f.scope, &reply_packet(request.call_id, bytes), 0).unwrap();
    assert!(events.is_empty());
    let mut replies = f.backend.take_im_replies(); assert_eq!(replies.len(), 1); replies.pop().unwrap()
}
fn cursor(f: &mut Fixture, target: i64) -> CursorSession {
    let reply = read_reply(f, Arc::new(ImQuery::refresh(&[], -1).unwrap()), &refresh_result(target));
    let mut cursor = CursorSession::from_refresh(f.run.clone(), f.gate.clone(), &reply).unwrap();
    assert!(cursor.observe_refresh(&f.temp.database, &reply).unwrap()); assert!(cursor.initial()); cursor
}
fn position(f: &Fixture) -> (i64, i64, i64) {
    f.temp.database.with_conn(|conn| conn.query_row(
        "SELECT baseline,consumed,target FROM app_voice_im_receive_cursors WHERE line_id=?1 AND box_id=?2",
        rusqlite::params![f.run.line_id(), b"P2P/1_2".as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(|_| "fixture")).unwrap()
}
// Every replacement binding still comes from the original pending native login.
// Never construct ImBinding from a caller UID or manufacture a permit from SQL.
fn next_native_login(f: &mut Fixture, receive_info: bool, selector: &'static [u8]) {
    f.scope.close();
    let cfg = NativeReceiverConfig {
        direct_domains: None, invite_notifications: None,
        im_messages: Some(NativeImConfig { notification_timeout_ms: 30000, receive_info, directory_queries: false }),
    };
    let mut backend = OfflineBackend::new(f.owner.clone());
    backend.claim_common_transport().unwrap(); backend.bind_common_identity(b"AccountEntry").unwrap();
    let scope = backend.connection_scope();
    backend.arm_native_receivers(NativeReceiverSetup::capture(NativeReceiverPolicy::capture(&cfg).unwrap(), true, None).unwrap()).unwrap();
    let mut inputs = login(); inputs.selected_session_id = selector;
    let request = backend.prepare_login(inputs, &[]).unwrap();
    backend.on_plaintext_packet(&f.owner, &scope, &reply_packet(request.call_id, &login_result()), 0).unwrap();
    backend.configure_native_receivers(7).unwrap(); f.scope = scope; f.backend = backend;
}

#[test]
fn info_runtime_policy_is_strict_omitted_by_default_and_captured_only_by_current_binding() {
    let absent: NativeReceiverConfig = serde_json::from_str("{}").unwrap();
    assert!(absent.im_messages.is_none());
    let default: NativeImConfig = serde_json::from_str(r#"{"notification_timeout_ms":30000}"#).unwrap();
    assert!(!default.receive_info);
    assert_eq!(serde_json::to_value(default).unwrap(), serde_json::json!({"notification_timeout_ms":30000}));
    let explicit: NativeImConfig = serde_json::from_str(r#"{"notification_timeout_ms":30000,"receive_info":false}"#).unwrap();
    assert_eq!(serde_json::to_value(explicit).unwrap(), serde_json::to_value(default).unwrap());
    let enabled: NativeImConfig = serde_json::from_str(r#"{"notification_timeout_ms":30000,"receive_info":true}"#).unwrap();
    assert_eq!(serde_json::to_value(enabled).unwrap()["receive_info"], true);
    for value in ["null", "0", "1", r#""true""#, "[]", "{}"] {
        let json = format!(r#"{{"notification_timeout_ms":30000,"receive_info":{value}}}"#);
        assert!(serde_json::from_str::<NativeImConfig>(&json).is_err());
        assert!(serde_json::from_str::<NativeReceiverConfig>(&format!(r#"{{"im_messages":{json}}}"#)).is_err());
    }
    for extra in [r#""uid":"forged""#, r#""selector":"forged""#, r#""grant":true"#] {
        assert!(serde_json::from_str::<NativeImConfig>(&format!(r#"{{"notification_timeout_ms":30000,"receive_info":true,{extra}}}"#)).is_err());
    }
    for (receivers, receive_info) in [(false, false), (true, false), (true, true)] {
        let mut f = Fixture::with_info(receivers, receive_info);
        let stored = f.temp.database.with_conn(|conn| account_store::load(conn, f.run.line_id())).unwrap().unwrap();
        let bytes = stored.config.encode_private().unwrap();
        let json = std::str::from_utf8(&bytes).unwrap();
        assert_eq!(json.contains("im_messages"), receivers);
        assert_eq!(json.contains("receive_info"), receivers && receive_info);
        if receivers {
            let delivery = info(&mut f, 1, b"P2P/1_2", 7, b"Info.Policy", INFO);
            let binding = delivery.notice().binding();
            assert_eq!(binding.receives_info(), receive_info);
            assert!(binding.matches(&f.owner, &f.scope));
            assert!(Arc::ptr_eq(binding, delivery.ticket().binding()));
            let captured = ImContent::capture(binding, &delivery.notice().message().unwrap().unwrap()).unwrap();
            assert_eq!(captured.is_some(), receive_info);
        } else {
            let events = f.receive(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &more(b"P2P/1_2", 7, b"Info.Policy", INFO), None);
            assert_eq!(control_result(&events), [0; 6]);
        }
        assert_eq!(f.count(), 0); no_other_grants(&f);
    }
}

#[test]
fn info_runtime_original_direct_ticket_requires_commit_then_exact_six_byte_native_result() {
    let mut f = Fixture::with_info(true, true);
    let rejected = info(&mut f, 1, b"Org/1-2", 6, b"Info.Direct", INFO);
    assert_eq!(f.count(), 0);
    assert_eq!(result(f.backend.complete_im(rejected.ticket(), None).unwrap()), [0; 6]);
    let message = info(&mut f, 2, b"Org/1-2", 0x1_2345_6789, b"Info.Direct", INFO);
    assert!(message.matches(&f.owner, &f.scope));
    assert!(message.notice().notification_id().is_none());
    let ImReplySource::Direct(original) = message.ticket().source() else { panic!("original DIRECT IM brand") };
    assert!(f.backend.complete_direct_message(original, true).is_err());
    assert_eq!(f.count(), 0); assert_eq!(event_count(&f), 0); assert!(f.backend.im_write_guard().0.is_empty());
    let deadline = message.ticket().completion_deadline();
    let proof = f.inbox(&message).commit(&f.temp.database, &message).unwrap().unwrap();
    assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    assert!(proof.authorizes(message.ticket(), Instant::now()));
    assert_eq!(message.ticket().completion_deadline(), deadline);
    assert_eq!(views(&f)[0].message_index, "4886718345");
    assert_eq!(result(f.backend.complete_im(message.ticket(), Some(&proof)).unwrap()), [0, 1, 0, 0, 0, 0]);
    assert!(f.backend.im_write_guard().0.is_empty()); assert!(f.scope.is_open()); no_other_grants(&f);
}

#[test]
fn info_runtime_system_info_is_typed_data_not_text_and_five_common_fields_stay_private() {
    let mut f = Fixture::with_info(true, true);
    for (rpc, kind, system) in [(1, b"Info.Text".as_slice(), false), (2, b"System.Info.Text".as_slice(), true)] {
        let message = info(&mut f, rpc, b"P2P/1_2", rpc as i64, kind, PRIVATE_INFO);
        let decoded = message.notice().message().unwrap().unwrap();
        assert!(ImText::decode(&decoded).unwrap().is_none());
        let data = ImInfo::decode(&decoded).unwrap().unwrap();
        assert_eq!(data.kind(), b"Text"); assert_eq!(data.system(), system); assert_eq!(data.content(), "visible info");
        assert!(data.field(TextField::Text).is_none());
        for (field, value) in COMMON_INFO { assert_eq!(data.field(field), Some(value)); }
        let normalized = ImContent::capture(message.notice().binding(), &decoded).unwrap().unwrap();
        assert!(matches!(&normalized, ImContent::Info(_)));
        assert_eq!(normalized.body_kind(), "info"); assert_eq!(normalized.info_type(), Some(b"Text".as_slice()));
        let proof = f.inbox(&message).commit(&f.temp.database, &message).unwrap().unwrap();
        let facts: (String, Vec<u8>, i64, Vec<u8>, String, Vec<u8>, i64) = f.temp.database.with_conn(|conn| conn.query_row(
            "SELECT body_kind,info_type,system_info,body,text,sender,attachment_count FROM app_voice_im_messages WHERE message_index=?1",
            [rpc as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?))).map_err(|_| "fixture")).unwrap();
        assert_eq!(facts.0, "info"); assert_eq!(facts.1, b"Text"); assert_eq!(facts.2, i64::from(system));
        assert_eq!(facts.3, message.notice().raw().unwrap()); assert_eq!(facts.4, "visible info");
        assert_eq!(facts.5, b"private-sender-uid"); assert_eq!(facts.6, 1);
        private_projection(&format!("{message:?} {:?} {:?} {data:?} {normalized:?} {proof:?} {:?}", message.ticket(), message.notice(), message.notice().binding()));
        assert_eq!(result(f.backend.complete_im(message.ticket(), Some(&proof)).unwrap()), [0, 1, 0, 0, 0, 0]);
    }
    let rows = views(&f); assert_eq!(rows.len(), 2);
    for (row, system) in rows.iter().zip([true, false]) {
        assert_eq!(row.kind, Some("info")); assert_eq!(row.info_type.as_deref(), Some("Text"));
        assert_eq!(row.system_info, Some(system)); assert_eq!(row.text, "visible info");
    }
    private_projection(&serde_json::to_string(&rows).unwrap());
    for event in f.temp.database.get_recent_app_events(Some(f.run.line_id()), 16).unwrap() {
        assert_eq!(event.event_type, "im.received"); private_projection(&event.payload_json);
        let value: serde_json::Value = serde_json::from_str(&event.payload_json).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3); assert_eq!(value["kind"], "info");
        assert_eq!(value["transport"], "app_voice_im");
    }
    no_other_grants(&f);
}

#[test]
fn info_runtime_default_false_has_no_info_sql_or_proof_nacks_and_keeps_scope_and_gap_healthy() {
    let mut f = Fixture::new(true);
    for (rpc, kind) in [(1, b"Info.Default".as_slice()), (2, b"System.Info.Default".as_slice())] {
        let message = info(&mut f, rpc, b"P2P/1_2", 7, kind, INFO);
        assert!(!message.notice().binding().receives_info());
        assert!(f.inbox(&message).commit(&f.temp.database, &message).unwrap().is_none());
        assert_eq!(result(f.backend.complete_im(message.ticket(), None).unwrap()), [0; 6]);
    }
    let notification = f.hint(3, 99, b"P2P/1_2", 7);
    assert!(f.inbox(&notification).lookup(&f.temp.database, &notification).unwrap().is_none());
    assert_eq!(result(f.backend.complete_im(notification.ticket(), None).unwrap()), [0; 6]);
    let c = cursor(&mut f, 2); let plan = c.next_gap(&f.temp.database).unwrap().unwrap();
    let reply = read_reply(&mut f, plan.query(), &recv_result(&[(1, b"Text", br#"{"text":"one"}"#), (2, b"Info.Default", INFO)]));
    assert_eq!(c.commit_gap(&f.temp.database, &plan, &reply).err(), Some("app_account_im_type_unsupported"));
    assert_eq!(position(&f), (0, 0, 2)); assert_eq!(f.count(), 0); assert_eq!(event_count(&f), 0);
    let text = f.message(4, b"Org/1-2", 7, r#"{"text":"legacy text"}"#);
    let proof = f.inbox(&text).commit(&f.temp.database, &text).unwrap().unwrap();
    assert_eq!(result(f.backend.complete_im(text.ticket(), Some(&proof)).unwrap()), [0, 1, 0, 0, 0, 0]);
    assert_eq!(views(&f)[0].kind, None); assert!(f.scope.is_open()); no_other_grants(&f);
}

#[test]
fn info_runtime_duplicates_are_idempotent_but_changed_same_identity_box_index_rolls_back() {
    let mut f = Fixture::with_info(true, true);
    let first = info(&mut f, 1, b"P2P/1_2", 2, b"Info.Saved", INFO);
    let proof = f.inbox(&first).commit(&f.temp.database, &first).unwrap().unwrap();
    let duplicate = info(&mut f, 2, b"P2P/1_2", 2, b"Info.Saved", INFO);
    let again = f.inbox(&duplicate).commit(&f.temp.database, &duplicate).unwrap().unwrap();
    assert!(proof.authorizes(duplicate.ticket(), Instant::now())); assert!(again.authorizes(first.ticket(), Instant::now()));
    assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    for (rpc, kind, body) in [
        (3, b"Info.Saved".as_slice(), br#"{"content":"changed"}"#.as_slice()),
        (4, b"Info.Other".as_slice(), INFO), (5, b"System.Info.Saved".as_slice(), INFO),
        (6, b"Info.Saved".as_slice(), br#"{"content":"visible info","userData":"changed"}"#.as_slice()),
    ] {
        let changed = info(&mut f, rpc, b"P2P/1_2", 2, kind, body);
        assert!(!proof.authorizes(changed.ticket(), Instant::now()));
        assert_eq!(f.inbox(&changed).commit(&f.temp.database, &changed).err(), Some("app_account_im_message_conflict"));
        assert_eq!(result(f.backend.complete_im(changed.ticket(), None).unwrap()), [0; 6]);
    }
    let c = cursor(&mut f, 2); let plan = c.next_gap(&f.temp.database).unwrap().unwrap();
    let reply = read_reply(&mut f, plan.query(), &recv_result(&[(1, b"Text", br#"{"text":"must roll back"}"#), (2, b"Info.Saved", br#"{"content":"changed"}"#)]));
    assert_eq!(c.commit_gap(&f.temp.database, &plan, &reply).err(), Some("app_account_im_message_conflict"));
    assert_eq!(position(&f), (0, 0, 2)); assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    let rows = views(&f); assert_eq!(rows[0].info_type.as_deref(), Some("Saved")); assert_eq!(rows[0].text, "visible info");
}

#[test]
fn info_runtime_hint_needs_exact_box_index_and_later_lookup_only_same_current_native_context() {
    for receive_info in [true, false] {
        let mut f = Fixture::with_info(true, true);
        let notification = f.hint(1, 0xf123_4567_89ab_cdef, b"P2P/1_2", 7);
        assert!(f.inbox(&notification).lookup(&f.temp.database, &notification).unwrap().is_none());
        for (rpc, box_id, index) in [(2, b"Org/1-2".as_slice(), 7), (3, b"P2P/1_2".as_slice(), 8)] {
            let wrong = info(&mut f, rpc, box_id, index, b"Info.Hint", INFO);
            let proof = f.inbox(&wrong).commit(&f.temp.database, &wrong).unwrap().unwrap();
            assert!(!proof.authorizes(notification.ticket(), Instant::now()));
            assert!(f.backend.complete_im(notification.ticket(), Some(&proof)).is_err());
            assert!(f.inbox(&notification).lookup(&f.temp.database, &notification).unwrap().is_none());
        }
        let right = info(&mut f, 4, b"P2P/1_2", 7, b"Info.Hint", INFO);
        assert!(Arc::ptr_eq(right.notice().binding(), notification.notice().binding()));
        let proof = f.inbox(&right).commit(&f.temp.database, &right).unwrap().unwrap();
        assert!(proof.authorizes(notification.ticket(), Instant::now()));
        assert_eq!(result(f.backend.complete_im(notification.ticket(), Some(&proof)).unwrap()), [0, 1, 0, 0, 0, 0]);
        let later = f.hint(5, 100, b"P2P/1_2", 7);
        let context = f.inbox(&later);
        let looked_up = context.lookup(&f.temp.database, &later).unwrap().unwrap();
        assert!(looked_up.authorizes(later.ticket(), Instant::now()));
        assert_eq!(f.count(), 3); assert_eq!(event_count(&f), 3);
        let original_binding = right.notice().binding().clone();
        next_native_login(&mut f, receive_info, b"fixture-selector");
        let foreign_context = f.hint(6, 101, b"P2P/1_2", 7);
        assert_eq!(original_binding.identity(), foreign_context.notice().binding().identity());
        assert_ne!(original_binding.receipt_epoch(), foreign_context.notice().binding().receipt_epoch());
        assert!(!looked_up.authorizes(foreign_context.ticket(), Instant::now()));
        assert!(context.lookup(&f.temp.database, &foreign_context).is_err());
        // Regression: a new original login/config must not promote an old row
        // into a current receipt merely because UID/selector/box/index match.
        assert!(f.inbox(&foreign_context).lookup(&f.temp.database, &foreign_context).unwrap().is_none());
        assert_eq!(result(f.backend.complete_im(foreign_context.ticket(), None).unwrap()), [0; 6]);
        assert_eq!(f.count(), 3); assert_eq!(event_count(&f), 3); assert!(f.scope.is_open());
    }
}

#[test]
fn info_runtime_delete_wipes_private_content_and_info_metadata_without_hash_resurrection() {
    let mut f = Fixture::with_info(true, true);
    let first = info(&mut f, 1, b"P2P/1_2", 7, b"System.Info.Deleted", PRIVATE_INFO);
    let proof = f.inbox(&first).commit(&f.temp.database, &first).unwrap().unwrap();
    let row = views(&f).pop().unwrap();
    let original_hash = first.notice().message_fingerprint().unwrap();
    assert!(f.temp.database.with_conn(|conn| n::im_inbox::delete(conn, f.run.line_id(), row.id)).unwrap());
    assert!(!f.temp.database.with_conn(|conn| n::im_inbox::delete(conn, f.run.line_id(), row.id)).unwrap());
    let tombstone: (Vec<u8>, String, Vec<u8>, i64, Option<Vec<u8>>, i64, Vec<u8>, i64) = f.temp.database.with_conn(|conn| conn.query_row(
        "SELECT body,text,sender,attachment_count,info_type,system_info,fingerprint,deleted FROM app_voice_im_messages WHERE id=?1",
        [row.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))).map_err(|_| "fixture")).unwrap();
    assert!(tombstone.0.is_empty()); assert!(tombstone.1.is_empty()); assert!(tombstone.2.is_empty());
    assert_eq!(tombstone.3, 0); assert!(tombstone.4.is_none()); assert_eq!(tombstone.5, 0);
    assert_eq!(tombstone.6.as_slice(), original_hash.as_slice()); assert_eq!(tombstone.7, 1);
    let replay = info(&mut f, 2, b"P2P/1_2", 7, b"System.Info.Deleted", PRIVATE_INFO);
    let replay_proof = f.inbox(&replay).commit(&f.temp.database, &replay).unwrap().unwrap();
    assert!(proof.authorizes(replay.ticket(), Instant::now())); assert!(replay_proof.authorizes(replay.ticket(), Instant::now()));
    let notification = f.hint(3, 99, b"P2P/1_2", 7);
    let receipt = f.inbox(&notification).lookup(&f.temp.database, &notification).unwrap().unwrap();
    assert_eq!(result(f.backend.complete_im(notification.ticket(), Some(&receipt)).unwrap()), [0, 1, 0, 0, 0, 0]);
    let changed = info(&mut f, 4, b"P2P/1_2", 7, b"Info.Deleted", PRIVATE_INFO);
    assert_eq!(f.inbox(&changed).commit(&f.temp.database, &changed).err(), Some("app_account_im_message_conflict"));
    assert!(views(&f).is_empty()); assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    let retained: i64 = f.temp.database.with_conn(|conn| conn.query_row(
        "SELECT length(body)+length(CAST(text AS BLOB))+length(sender)+COALESCE(length(info_type),0)+attachment_count+system_info FROM app_voice_im_messages",
        [], |r| r.get(0)).map_err(|_| "fixture")).unwrap(); assert_eq!(retained, 0);
}

#[test]
fn info_runtime_event_failure_and_info_type_byte_budget_roll_back_records_and_whole_gap() {
    let mut f = Fixture::with_info(true, true);
    let c = cursor(&mut f, 3);
    f.temp.database.with_conn(|conn| conn.execute_batch(
        "CREATE TRIGGER fail_info BEFORE INSERT ON app_events WHEN NEW.event_type='im.received' BEGIN SELECT RAISE(ABORT,'fixture'); END"
    ).map_err(|_| "fixture")).unwrap();
    let direct = info(&mut f, 1, b"P2P/1_2", 1, b"Info.Atomic", INFO);
    assert!(f.inbox(&direct).commit(&f.temp.database, &direct).is_err());
    assert_eq!(result(f.backend.complete_im(direct.ticket(), None).unwrap()), [0; 6]);
    let plan = c.next_gap(&f.temp.database).unwrap().unwrap();
    let reply = read_reply(&mut f, plan.query(), &recv_result(&[
        (1, b"Text", br#"{"text":"one"}"#), (2, b"Info.Atomic", INFO), (3, b"System.Info.Atomic", INFO),
    ]));
    assert!(c.commit_gap(&f.temp.database, &plan, &reply).is_err());
    assert_eq!(f.count(), 0); assert_eq!(event_count(&f), 0); assert_eq!(position(&f), (0, 0, 3));
    assert!(f.scope.is_open());
    // Occupancy-only SQL fixture: it grants nothing and has a different identity.
    // Leave exactly the new raw+preview+sender size free, but NOT its subtype bytes.
    let mut full = Fixture::with_info(true, true);
    let kind = [b"Info.".as_slice(), vec![b'A'; 128].as_slice()].concat();
    let message = info(&mut full, 1, b"P2P/1_2", 7, &kind, INFO);
    let decoded = message.notice().message().unwrap().unwrap();
    let without_type = message.notice().raw().unwrap().len() + "visible info".len() + decoded.sender_uid.len();
    full.temp.database.with_conn(|conn| conn.execute(
        "WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<128)
         INSERT INTO app_voice_im_messages(line_id,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at)
         SELECT ?1,1,zeroblob(32),CAST('P2P/9_9' AS BLOB),'p2p',n,zeroblob(32),0,zeroblob(65536),
                CAST(zeroblob(CASE WHEN n=128 THEN 65536-?2 ELSE 65536 END) AS TEXT),X'',0,'fixture' FROM ids",
        rusqlite::params![full.run.line_id(), without_type as i64]).map_err(|_| "fixture")).unwrap();
    let bytes: i64 = full.temp.database.with_conn(|conn| conn.query_row(
        "SELECT SUM(length(body)+length(CAST(text AS BLOB))+length(sender)+COALESCE(length(info_type),0)) FROM app_voice_im_messages",
        [], |r| r.get(0)).map_err(|_| "fixture")).unwrap();
    assert_eq!(bytes + without_type as i64, 16 * 1024 * 1024);
    assert_eq!(full.inbox(&message).commit(&full.temp.database, &message).err(), Some("app_account_im_inbox_full"));
    assert_eq!(full.count(), 128); assert_eq!(event_count(&full), 0);
    assert_eq!(result(full.backend.complete_im(message.ticket(), None).unwrap()), [0; 6]); no_other_grants(&full);
}

#[test]
fn info_runtime_foreign_run_identity_scope_and_exact_type_body_cannot_redeem_proof() {
    let mut f = Fixture::with_info(true, true); let mut other = Fixture::with_info(true, true);
    let first = info(&mut f, 1, b"P2P/1_2", 7, b"Info.Owner", INFO);
    let proof = f.inbox(&first).commit(&f.temp.database, &first).unwrap().unwrap();
    let foreign = info(&mut other, 1, b"P2P/1_2", 7, b"Info.Owner", INFO);
    assert!(!proof.authorizes(foreign.ticket(), Instant::now()));
    assert!(other.backend.complete_im(foreign.ticket(), Some(&proof)).is_err());
    assert!(f.inbox(&first).commit(&f.temp.database, &foreign).is_err()); assert_eq!(other.count(), 0);
    let nonce = local_nonce().unwrap();
    let non_runtime = Arc::new(f.temp.database.with_conn(|conn| account_runs::register(conn, f.run.line_id(), 1, &nonce)).unwrap());
    let wrong_run = ImInboxScope::original(non_runtime, first.notice().binding().clone(), f.gate.clone()).unwrap();
    assert!(wrong_run.commit(&f.temp.database, &first).is_err());
    let notice = f.hint(2, 99, b"P2P/1_2", 7); assert!(wrong_run.lookup(&f.temp.database, &notice).is_err());
    for (rpc, kind, body) in [(3, b"System.Info.Owner".as_slice(), INFO), (4, b"Info.Owner".as_slice(), br#"{"content":"other"}"#.as_slice()), (5, b"Text".as_slice(), br#"{"text":"visible info"}"#.as_slice())] {
        let changed = info(&mut f, rpc, b"P2P/1_2", 7, kind, body);
        assert!(!proof.authorizes(changed.ticket(), Instant::now()));
        assert!(f.backend.complete_im(changed.ticket(), Some(&proof)).is_err());
    }
    let arguments = more(b"P2P/1_2", 7, b"Info.Owner", INFO);
    let raw = encode_request(6, &[RpcProperty { key: b"id", value: b"CallClientEx" }], n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &arguments).unwrap();
    let request = decode_request(raw.expose_to_transport()).unwrap();
    assert!(n::im_delivery::direct_message(first.notice().binding(), &f.owner, &f.scope, &request).is_err());
    assert!(n::im_delivery::direct_message(first.notice().binding(), &other.owner, &other.scope, &request).unwrap().is_none());
    next_native_login(&mut f, true, b"fixture-other-selector");
    let different_identity = f.hint(7, 100, b"P2P/1_2", 7);
    assert_ne!(first.notice().binding().identity(), different_identity.notice().binding().identity());
    assert!(!proof.authorizes(different_identity.ticket(), Instant::now()));
    assert!(f.inbox(&different_identity).lookup(&f.temp.database, &different_identity).unwrap().is_none());
    assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
}

#[test]
fn info_runtime_retirement_close_revoke_and_clipped_deadline_never_become_positive() {
    for mode in 0..4 {
        let mut f = Fixture::with_info(true, true);
        let message = info(&mut f, 1, b"P2P/1_2", 7, b"Info.Lifetime", INFO);
        let proof = f.inbox(&message).commit(&f.temp.database, &message).unwrap().unwrap();
        match mode {
            0 => { f.gate.retire(); assert!(!proof.authorizes(message.ticket(), Instant::now()));
                assert!(f.backend.complete_im(message.ticket(), Some(&proof)).is_err()); assert!(f.scope.is_open()); }
            1 => { f.scope.close(); assert!(!proof.authorizes(message.ticket(), Instant::now()));
                assert!(f.backend.complete_im(message.ticket(), Some(&proof)).unwrap().is_none()); }
            2 => { f.scope.mark_session_revoked(); assert!(!proof.authorizes(message.ticket(), Instant::now()));
                assert!(f.backend.complete_im(message.ticket(), Some(&proof)).unwrap().is_none()); }
            _ => { let expired = message.ticket().clipped_to(Instant::now());
                let extended = expired.clipped_to(Instant::now() + Duration::from_secs(60));
                assert_eq!(extended.completion_deadline(), expired.completion_deadline());
                assert_eq!(expired.deadline(), message.ticket().deadline());
                assert!(!proof.authorizes(&expired, Instant::now())); assert!(!proof.authorizes(&extended, Instant::now()));
                assert!(f.backend.complete_im(&extended, Some(&proof)).is_err());
                assert_eq!(result(f.backend.complete_im(&expired, None).unwrap()), [0; 6]); assert!(f.scope.is_open()); }
        }
        assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    }
    let mut f = Fixture::with_info(true, true);
    let queued = batch(&mut f, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &more(b"P2P/1_2", 7, b"Info.Queued", INFO));
    let proof = f.inbox(&queued).commit(&f.temp.database, &queued).unwrap().unwrap();
    f.gate.retire(); assert!(!proof.authorizes(queued.ticket(), Instant::now()));
    assert!(f.backend.complete_im(queued.ticket(), Some(&proof)).is_err());
    assert!(f.backend.im_write_guard().0.is_empty()); assert!(f.scope.is_open());
    assert!(matches!(f.backend.complete_im(queued.ticket(), None).unwrap(), Some(n::offline_backend::CallCompletion::BatchQueued)));
    assert!(f.scope.is_open());
}

#[test]
fn info_runtime_strict_batch_uses_original_result_brand_retained_until_native_batch_ack() {
    let mut f = Fixture::with_info(true, true);
    let message = batch(&mut f, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &more(b"P2P/1_2", 7, b"System.Info.Batch", INFO));
    let ImReplySource::Batch(inner) = message.ticket().source() else { panic!("original batch IM brand") };
    assert!(f.backend.complete_message(inner, true).is_err()); assert!(f.backend.complete_message(inner, false).is_err());
    assert_eq!(f.count(), 0); assert!(f.backend.im_write_guard().0.is_empty());
    let proof = f.inbox(&message).commit(&f.temp.database, &message).unwrap().unwrap();
    assert!(matches!(f.backend.complete_im(message.ticket(), Some(&proof)).unwrap(), Some(n::offline_backend::CallCompletion::BatchQueued)));
    let (gates, deadline) = f.backend.im_write_guard(); assert_eq!(gates.len(), 1);
    assert!(Arc::ptr_eq(&gates[0], &f.gate)); assert_eq!(deadline, Some(message.ticket().completion_deadline()));
    let request = f.backend.prepare_swap(2, &[]).unwrap();
    let outer = decode_request(&request.packets[0].expose_to_transport()[1..]).unwrap();
    let mut reader = Reader::versioned(outer.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
    let payload = n::rpc_batch::read_payload(&mut reader).unwrap();
    assert_eq!(payload.results.len(), 1); assert_eq!(payload.results[&1].result_stream, [0, 1, 0, 0, 0, 0]);
    assert_eq!(f.backend.im_write_guard().0.len(), 1); // serialization/submission is NOT native batch ACK
    let ack = n::runtime_backend::tests::swap_result(None, true, payload.fields);
    f.backend.on_plaintext_packet(&f.owner, &f.scope, &reply_packet(outer.call_id, &ack), 3).unwrap();
    let (gates, deadline) = f.backend.im_write_guard(); assert!(gates.is_empty()); assert!(deadline.is_none());
    assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1); assert!(f.scope.is_open()); no_other_grants(&f);
}

#[test]
fn info_runtime_legacy_schema_is_additive_text_default_and_info_projection_rejects_broken_metadata() {
    let mut f = Fixture::with_info(true, true);
    let text = f.message(1, b"Org/1-2", 7, r#"{"text":"legacy text","content":"not inferred as info"}"#);
    f.inbox(&text).commit(&f.temp.database, &text).unwrap();
    // Remove only the new columns to reproduce the complete old schema/row.
    f.temp.database.with_conn(|conn| conn.execute_batch(
        "ALTER TABLE app_voice_im_messages DROP COLUMN body_kind;
         ALTER TABLE app_voice_im_messages DROP COLUMN info_type;
         ALTER TABLE app_voice_im_messages DROP COLUMN system_info;
         ALTER TABLE app_voice_im_messages DROP COLUMN info_epoch;"
    ).map_err(|_| "fixture")).unwrap();
    f.temp.database.with_conn(n::im_inbox::initialize_schema).unwrap();
    f.temp.database.with_conn(n::im_inbox::initialize_schema).unwrap(); // idempotent migration
    let legacy: (String, Option<Vec<u8>>, i64, Vec<u8>) = f.temp.database.with_conn(|conn| conn.query_row(
        "SELECT body_kind,info_type,system_info,fingerprint FROM app_voice_im_messages", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|_| "fixture")).unwrap();
    assert_eq!(legacy.0, "text"); assert!(legacy.1.is_none()); assert_eq!(legacy.2, 0);
    assert_eq!(legacy.3.as_slice(), text.notice().message_fingerprint().unwrap().as_slice());
    let rows = views(&f); assert_eq!(rows.len(), 1); assert_eq!(rows[0].text, "legacy text");
    let json = serde_json::to_value(&rows[0]).unwrap();
    for field in ["kind", "info_type", "system_info"] { assert!(json.get(field).is_none()); }
    no_other_grants(&f);
    let c = cursor(&mut f, 3); let plan = c.next_gap(&f.temp.database).unwrap().unwrap();
    let invalid = read_reply(&mut f, plan.query(), &recv_result(&[
        (1, b"Text", br#"{"text":"one"}"#), (2, b"Info.Typed", br#"{"content":"ok","OnlineMsg":true}"#),
    ]));
    assert_eq!(c.commit_gap(&f.temp.database, &plan, &invalid).err(), Some("app_account_im_message_invalid"));
    assert_eq!(position(&f), (0, 0, 3)); assert_eq!(f.count(), 1); assert_eq!(event_count(&f), 1);
    let valid = read_reply(&mut f, plan.query(), &recv_result(&[
        (1, b"Text", br#"{"text":"one","content":false}"#), (2, b"Info.Typed", PRIVATE_INFO), (3, b"System.Info.Typed", INFO),
    ]));
    let committed = c.commit_gap(&f.temp.database, &plan, &valid).unwrap();
    assert_eq!(committed.end(), 3); assert_eq!(committed.proofs().len(), 3); assert!(committed.is_current(&f.temp.database));
    assert_eq!(position(&f), (0, 3, 3)); assert_eq!(f.count(), 4); assert_eq!(event_count(&f), 4);
    let rows = views(&f); private_projection(&serde_json::to_string(&rows).unwrap());
    let info_id = rows.iter().find(|row| row.kind == Some("info")).unwrap().id;
    for subtype in [b"bad/type".as_slice(), &[0xff]] {
        f.temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_im_messages SET info_type=?1 WHERE id=?2", rusqlite::params![subtype, info_id]).map_err(|_| "fixture")).unwrap();
        assert!(f.temp.database.with_conn(|conn| n::im_inbox::list(conn, f.run.line_id(), 128)).is_err());
    }
    f.temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_im_messages SET info_type=X'5479706564' WHERE id=?1", [info_id]).map_err(|_| "fixture")).unwrap();
    // Unknown type tags/storage classes/system flags cannot become public Info.
    for sql in [
        "UPDATE app_voice_im_messages SET body_kind='unknown' WHERE id=?1",
        "UPDATE app_voice_im_messages SET info_type='Typed' WHERE id=?1",
        "UPDATE app_voice_im_messages SET info_type=X'' WHERE id=?1",
        "UPDATE app_voice_im_messages SET system_info=2 WHERE id=?1",
    ] { assert!(f.temp.database.with_conn(|conn| conn.execute(sql, [info_id]).map_err(|_| "fixture")).is_err()); }
    // Nullable Info metadata is excluded, never silently reclassified as Text.
    f.temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_im_messages SET info_type=NULL WHERE id=?1", [info_id]).map_err(|_| "fixture")).unwrap();
    assert!(!views(&f).iter().any(|row| row.id == info_id));
    assert_eq!(f.count(), 4); assert_eq!(event_count(&f), 4); no_other_grants(&f);
}
