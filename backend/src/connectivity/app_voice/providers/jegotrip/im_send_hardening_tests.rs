//! Actions-only native sender accounting and deterministic ownership races.
//! No SDK/SO/device execution. Every spawned resolver/canceller is joined.
use super::*;
const NONCE:&str="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn end() -> Instant { Instant::now() + Duration::from_secs(30) }
fn row_count(f: &Fixture) -> i64 { f.temp.database.with_conn(|c| c.query_row("SELECT COUNT(*) FROM app_voice_im_outbox", [], |r| r.get(0)).map_err(|_| "fixture outbox")).unwrap() }

#[test]
fn send_hardening_input_shared_capacity_is_admitted_before_sql_or_any_original_id() {
    let mut f = Fixture::new(true);
    let full = f.scope.reserve_native_rpc(131072).unwrap();
    let body = ImSendMessage::text("original input").unwrap();
    assert_eq!(body.reserve_input(&f.scope, b"1_2", NONCE).err(), Some("app_account_im_send_capacity"));
    assert_eq!(row_count(&f), 0);
    assert_eq!(f.scope.native_rpc_resources(), (1, 131072));
    drop(full);
    let input = body.reserve_input(&f.scope, b"1_2", NONCE).unwrap();
    assert_eq!(f.scope.native_rpc_resources().0, 1);
    assert!(f.scope.native_rpc_resources().1 > "original input".len());
    drop(input);
    let read = f.backend.prepare_im_read(Arc::new(n::rpc_im::ImQuery::refresh(&[], -1).unwrap()), f.gate.clone(), end()).unwrap();
    assert_eq!(read.call_id, 2, "no input rejection can consume the native send ID");
    f.backend.retire_im_read(read.call_id);
    assert_eq!(f.scope.native_rpc_resources(), (0, 0));
}

#[test]
fn send_hardening_input_reservation_cannot_be_rebound_to_changed_message_nonce_or_scope() {
    for mode in 0..3 {
        let f = Fixture::new(true); let other = Fixture::new(true);
        let original = ImSendMessage::text("original input").unwrap();
        let scope = if mode == 2 { &other.scope } else { &f.scope };
        let input = original.reserve_input(scope, b"1_2", NONCE).unwrap();
        let body = if mode == 0 { ImSendMessage::text("changed input").unwrap() } else { original };
        let nonce = if mode == 1 { "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" } else { NONCE };
        assert!(ImSendAction::original_with_input(f.temp.database.clone(), f.run.clone(), f.gate.clone(),
            f.backend.captured_im_binding().unwrap(), f.backend.captured_send_identity().unwrap(),
            b"1_2", body, nonce, end(), &policy(), input).is_err());
        assert_eq!(row_count(&f), 0);
        assert_eq!(f.scope.native_rpc_resources(), (0, 0));
        assert_eq!(other.scope.native_rpc_resources(), (0, 0));
    }
}

#[test]
fn send_hardening_original_unknown_is_account_wide_not_mutable_uid_selector_identity() {
    let f = Fixture::new(true); let a = fresh(&f);
    f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_im_outbox SET identity_key=zeroblob(32)", []).map_err(|_| "fixture old binding")).unwrap();
    let error = ImSendAction::original(f.temp.database.clone(), f.run.clone(), f.gate.clone(),
        f.backend.captured_im_binding().unwrap(), f.backend.captured_send_identity().unwrap(),
        b"[phone:+12345@fixture.invalid]", ImSendMessage::text("fresh intent").unwrap(),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", end(), &policy()).err();
    assert_eq!(error, Some("app_account_im_send_uncertain"));
    assert_eq!(state(&f), (0, None, None));
    assert_eq!(row_count(&f), 1);
    assert!(f.scope.is_open());
    a.cancel();
}

#[test]
fn send_hardening_permit_arguments_are_charged_before_box_binding_and_never_partial_sql() {
    let f = Fixture::new(true); let a = fresh(&f);
    assert_eq!(f.scope.native_rpc_resources().0, 2, "input and sealed identity are distinct original holders");
    let before = f.scope.native_rpc_resources();
    let full = f.scope.reserve_native_rpc(131072 - before.1).unwrap();
    assert_eq!(a.permit(&policy()).err(), Some("app_account_im_send_capacity"));
    let unbound: bool = f.temp.database.with_conn(|c| c.query_row("SELECT box_id IS NULL AND arguments_hash IS NULL AND request_id IS NULL FROM app_voice_im_outbox", [], |r| r.get(0)).map_err(|_| "fixture unbound")).unwrap();
    assert!(unbound); assert_eq!(state(&f), (0, None, None));
    drop(full); a.cancel(); assert_eq!(state(&f), (3, None, None));
    assert_eq!(f.scope.native_rpc_resources(), before, "cancellation alone cannot refund the held input");
    drop(a); assert_eq!(f.scope.native_rpc_resources(), (0, 0)); assert!(f.scope.is_open());
}

#[test]
fn send_hardening_shared_entry_full_rejects_before_arm_id_and_next_exact_id_is_preserved() {
    let mut f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&policy()).unwrap();
    assert_eq!(f.scope.native_rpc_resources().0, 3);
    let mut held = Vec::new(); for _ in 0..125 { held.push(f.scope.reserve_native_rpc(0).unwrap()); }
    assert_eq!(f.scope.native_rpc_resources().0, 128);
    assert!(f.backend.prepare_im_send(p.clone(), end()).is_err());
    assert_eq!(state(&f), (0, None, None)); assert!(f.scope.is_open());
    drop(held.pop());
    let packet = write(&mut f, &p); assert_eq!(packet.call_id, 2);
    assert_eq!(f.scope.native_rpc_resources().0, 128);
    p.cancel(); f.backend.retire_im_send(packet.call_id);
    assert_eq!(f.scope.native_rpc_resources().0, 128, "the physical packet's original request holder remains charged");
    drop(packet); drop(p); drop(a); drop(held);
    assert_eq!(f.scope.native_rpc_resources(), (0, 0));
}

#[test]
fn send_hardening_wire_private_reply_input_arguments_remain_until_last_holder_after_close() {
    let mut f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&policy()).unwrap();
    let packet = write(&mut f, &p); assert_eq!(f.scope.native_rpc_resources().0, 4);
    assert!(p.begin_write(packet.call_id)); assert!(p.submitted());
    let reply = result(&mut f, packet.call_id, true, 0);
    let last_request = reply.request().clone();
    assert_eq!(p.resolve(&reply).unwrap().state, "confirmed");
    let charged = f.scope.native_rpc_resources();
    f.scope.close(); f.backend.retire_im_send(packet.call_id);
    drop(a); drop(p); drop(reply);
    assert_eq!(f.scope.native_rpc_resources(), charged, "close/take/drop actor reply does not reset original accounting");
    drop(packet);
    assert_eq!(f.scope.native_rpc_resources(), charged, "a last private request still owns wire/input/arguments");
    drop(last_request); assert_eq!(f.scope.native_rpc_resources(), (0, 0));
    assert_eq!(state(&f), (1, Some(2), Some(i64::MAX)));
}

#[test]
fn send_hardening_malformed_or_unproven_nonzero_result_never_resolves_unknown() {
    for (code, raw) in [(65537, vec![0]), (65537, vec![0, 1, 0, 0, 0, 0]),
        (65537, { let mut w=Writer::new();w.boolean(false).unwrap();w.i64(1).unwrap();w.i64(2).unwrap();w.bytes(b"").unwrap();let mut v=w.finish().expose_to_transport().to_vec();v.push(0);v }),
        (0xffff_ffff, { let mut w=Writer::new();w.boolean(false).unwrap();w.i64(1).unwrap();w.i64(2).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec() })] {
        let mut f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&policy()).unwrap();
        let packet = write(&mut f, &p); assert!(p.begin_write(packet.call_id)); assert!(p.submitted());
        let encoded = encode_reply(packet.call_id, code, &raw).unwrap();
        let mut wire = vec![1]; wire.extend_from_slice(encoded.expose_to_transport());
        assert!(f.backend.on_plaintext_packet(&f.owner, &f.scope, &wire, 1).is_err());
        assert!(f.backend.take_im_send_replies().is_empty());
        assert_eq!(state(&f), (0, Some(packet.call_id), None)); assert!(!f.scope.is_open());
        p.cancel(); drop(packet); drop(p); drop(a);
        assert_eq!(f.scope.native_rpc_resources(), (0, 0));
    }
}

#[test]
fn send_hardening_resolution_commit_wins_waiting_cancel_and_both_observe_terminal_scope_health() {
    let mut f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&policy()).unwrap();
    let packet = write(&mut f, &p); assert!(p.begin_write(packet.call_id)); assert!(p.submitted());
    let reply = result(&mut f, packet.call_id, true, 0);
    let (entered, entering) = std::sync::mpsc::sync_channel(1);
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let probe_release = release.clone();
    p.set_resolve_probe(Arc::new(move || {
        entered.send(()).unwrap();
        let (lock, changed) = &*probe_release;
        let ready = lock.lock().unwrap();
        let (ready, timeout) = changed.wait_timeout_while(ready, Duration::from_secs(3), |v| !*v).unwrap();
        assert!(*ready && !timeout.timed_out(), "bounded test commit barrier");
    }));
    let (resolved, result_ready) = std::sync::mpsc::sync_channel(1);
    let resolving = p.clone();
    let resolver = std::thread::spawn(move || { let value = resolving.resolve(&reply); resolved.send(value).unwrap(); });
    entering.recv_timeout(Duration::from_secs(3)).unwrap();
    let (started, starting) = std::sync::mpsc::sync_channel(1); let (done, done_read) = std::sync::mpsc::sync_channel(1);
    let cancelling = p.clone();
    let canceller = std::thread::spawn(move || { started.send(()).unwrap(); cancelling.cancel(); done.send(()).unwrap(); });
    starting.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(matches!(done_read.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    { let (lock, changed) = &*release; *lock.lock().unwrap() = true; changed.notify_all(); }
    assert_eq!(result_ready.recv_timeout(Duration::from_secs(3)).unwrap().unwrap().state, "confirmed");
    done_read.recv_timeout(Duration::from_secs(3)).unwrap();
    resolver.join().unwrap(); canceller.join().unwrap();
    assert_eq!(state(&f), (1, Some(2), Some(i64::MAX)));
    assert!(f.scope.is_open()); assert!(!p.closed_original_io());
    drop(packet); drop(p); drop(a); assert_eq!(f.scope.native_rpc_resources(), (0, 0));
}

#[test]
fn send_hardening_cancel_wins_before_original_resolution_and_sql_remains_unknown() {
    let mut f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&policy()).unwrap();
    let packet = write(&mut f, &p); assert!(p.begin_write(packet.call_id)); assert!(p.submitted());
    let reply = result(&mut f, packet.call_id, true, 0);
    p.cancel(); assert_eq!(p.resolve(&reply).err(), Some("app_account_im_send_uncertain"));
    assert_eq!(state(&f), (0, Some(2), None)); assert!(!f.scope.is_open()); assert!(p.closed_original_io());
    drop(reply); drop(packet); drop(p); drop(a); assert_eq!(f.scope.native_rpc_resources(), (0, 0));
}

#[test]
fn send_hardening_missing_and_explicit_empty_original_country_are_same_data_not_device_info() {
    let none = policy(); let empty = NativeImSendConfig { send_text:true, send_info:true, auth_country_code:Some(String::new()) };
    assert!(none.validate().is_ok()); assert!(empty.validate().is_ok());
    let f = Fixture::new(true); let a = fresh(&f); let p = a.permit(&empty).unwrap();
    let mut reader = Reader::versioned(p.arguments()).unwrap();
    reader.bytes().unwrap();reader.bytes().unwrap();reader.bytes().unwrap();reader.properties().unwrap();reader.properties().unwrap();
    let fields = reader.properties().unwrap();
    assert_eq!(fields.iter().find(|p|p.key==b"Statistics.CountryCode").unwrap().value, b"");
    assert!(!fields.iter().any(|p|p.key==b"DeviceInfo.CC"||p.key==b"Notify.CountryCode"));reader.finish().unwrap();
}
