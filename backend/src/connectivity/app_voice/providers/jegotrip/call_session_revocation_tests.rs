//! Actions-only native-token revocation must stop media before physical close.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
fn kick(rpc: &ArcRpcSession, original: &AttemptOwner, scope: &RpcConnectionScope) {
    let dispatcher = rpc.client_ex_dispatcher(n::rpc_incoming::IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"Sip.Request" }).unwrap();
    let mut w = Writer::versioned().unwrap(); w.bytes(b"same-device-login:fixture").unwrap(); let args = w.finish();
    let request = RpcRequest { call_id: 901, method: n::rpc_client_ex::NOTIFY_DISCONNECT_METHOD.as_bytes(), arguments: args.expose_to_transport(), metadata: vec![
        RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"token", value: b"fixture-auth-token" }] };
    assert!(dispatcher.on_direct(original, scope, &request).unwrap().is_some());
    assert!(scope.session_revoked()); assert!(scope.is_open(), "the one terminal control response still has its original physical writer");
}
#[test]
fn token_revocation_blocks_media_and_retained_positive_result_before_physical_reply_or_close() {
    let (mut rpc, original, scope, d) = context(); let incoming = notice(&d, &original, &scope, CallNoticeKind::Invited, ID);
    let ticket = rpc.register_call_notice(&incoming).unwrap().unwrap(); rpc.call_acknowledged(&ticket, true).unwrap();
    let (id, _) = rpc.prepare_call(&ticket, CallOperation::Inform, &[]).unwrap(); let answer = finish(&mut rpc, &original, &scope, id, CallOperation::Inform, true);
    assert!(answer.properties_for(&original, &scope).is_some()); assert_eq!(ticket.with_media_gate(|| 1), Some(1));
    kick(&rpc, &original, &scope);
    assert!(!ticket.is_live()); assert_eq!(ticket.with_media_gate(|| panic!("revoked media poll")), None::<()>);
    assert!(!ticket.begin_media()); assert!(answer.properties_for(&original, &scope).is_none());
    assert!(!ticket.remote_released(), "a session revocation is not a fabricated call-release acknowledgement");
    assert!(scope.with_io_gate(|| ()).is_some()); assert!(rpc.prepare_call(&ticket, CallOperation::Accept, &[]).is_err());
}
#[tokio::test]
async fn token_revocation_wakes_both_existing_and_later_media_waiters_without_waiting_for_physical_eof() {
    let (mut rpc, original, scope, d) = context(); let incoming = notice(&d, &original, &scope, CallNoticeKind::Invited, ID);
    let ticket = rpc.register_call_notice(&incoming).unwrap().unwrap(); let copy = ticket.captured_copy();
    let waiting = tokio::spawn(async move { copy.revoked().await }); tokio::task::yield_now().await;
    let (mut other, other_owner, other_scope, other_d) = context(); let other_notice = notice(&other_d, &other_owner, &other_scope, CallNoticeKind::Invited, ID);
    let other_ticket = other.register_call_notice(&other_notice).unwrap().unwrap();
    kick(&rpc, &original, &scope);
    tokio::time::timeout(std::time::Duration::from_secs(1), waiting).await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), ticket.revoked()).await.unwrap();
    assert!(scope.is_open()); assert!(other_ticket.is_live()); assert_eq!(other_ticket.with_media_gate(|| 7), Some(7));
}
#[test]
fn late_positive_call_reply_after_native_kick_cannot_reenable_media_or_consume_original_pending() {
    let (mut rpc, original, scope, d) = context(); let incoming = notice(&d, &original, &scope, CallNoticeKind::Invited, ID);
    let ticket = rpc.register_call_notice(&incoming).unwrap().unwrap(); rpc.call_acknowledged(&ticket, true).unwrap();
    let (id, _) = rpc.prepare_call(&ticket, CallOperation::Inform, &[]).unwrap(); let payload = result(CallOperation::Inform, true);
    let reply = encode_reply(id, 0, payload.expose_to_transport()).unwrap(); kick(&rpc, &original, &scope);
    assert!(rpc.on_reply(&original, &scope, reply.expose_to_transport()).unwrap().is_none()); assert!(rpc.is_pending(id));
    assert!(!ticket.is_live()); assert_eq!(ticket.with_media_gate(|| 1), None); assert!(scope.is_open());
    rpc.close(); assert!(!scope.is_open()); assert!(!rpc.is_pending(id));
}
