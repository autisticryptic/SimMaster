//! Actions-only sticky terminal provenance and zero new command admission.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{rpc_client_ex::{ClientExDispatcher, NOTIFY_DISCONNECT_METHOD},
    rpc_incoming::IncomingBinding, session::BackendFailure};

fn latched_scope(original: &AttemptOwner) -> RpcConnectionScope {
    let scope = RpcConnectionScope::fresh();
    let dispatcher = ClientExDispatcher::for_arc_session(original.clone(), scope.clone(),
        IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" }, b"fixture-auth").unwrap();
    let mut writer = Writer::versioned().unwrap(); writer.bytes(b"server-kick-off:fixture-private").unwrap();
    let bytes = crate::connectivity::app_voice::providers::jegotrip::rpc_envelope::encode_request(1, &[
        RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"token", value: b"fixture-auth" }], NOTIFY_DISCONNECT_METHOD.as_bytes(), writer.finish().expose_to_transport()).unwrap();
    dispatcher.on_direct(original, &scope, &decode_request(bytes.expose_to_transport()).unwrap()).unwrap().unwrap();
    assert!(scope.session_revoked()); scope
}
#[test]
fn terminal_lookup_retains_protocol_latched_revocation_without_a_queued_callback_and_rejects_other_scopes() {
    let original = owner(); let scope = latched_scope(&original); scope.close();
    let event = RuntimeEvent { owner: original.clone(), scope: scope.clone(), events: Vec::new() };
    assert_eq!(event.terminal_failure(&original, &scope), Some(BackendFailure::SessionRevoked));
    assert_eq!(event.terminal_failure(&owner(), &scope), None);
    assert_eq!(event.terminal_failure(&original, &RpcConnectionScope::fresh()), None);
    assert!(event.events_for(&original, &scope).is_none());
}
#[tokio::test]
async fn revoked_scope_blocks_new_commands_even_before_the_one_reply_write_and_physical_close() {
    let original = owner(); let scope = latched_scope(&original);
    assert!(scope.is_open(), "the owning actor alone may still finish its original control reply");
    let (sender, mut receiver) = mpsc::channel(MAX_COMMANDS);
    let commands = RuntimeCommands { owner: original.clone(), scope: scope.clone(), commands: sender };
    assert_eq!(commands.poll(&original, &scope, &[]).await.err(), Some(RuntimeError::Scope));
    assert_eq!(commands.configure_client_ex(&original, &scope).await.err(), Some(RuntimeError::Scope));
    assert!(receiver.try_recv().is_err(), "a revoked session cannot queue another business/poll/configuration command");
    let receipt = RuntimeReceipt::new(original.clone(), scope.clone(), Some(1));
    assert!(!receipt.matches(&original, &scope)); assert!(receipt.submitted_on(&original, &scope));
    assert!(scope.with_io_gate(|| ()).is_some()); scope.close(); assert!(scope.with_io_gate(|| ()).is_none());
}
