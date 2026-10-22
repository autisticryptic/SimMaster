use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    rpc_envelope::decode_request, session::{HostingSession, Phase}, tests::imported,
};

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    session.begin_login(100).unwrap()
}

fn registration<'a>() -> GatewayRegistration<'a> {
    GatewayRegistration { authname_prefix: "prefix", channel_id: "channel", delete_int_code: "1",
        sip_number: "number", sip_password: "fixture-secret-password" }
}

fn reply(id: i32, code: u32, result: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&code.to_be_bytes());
    bytes.extend_from_slice(&(result.len() as u32).to_be_bytes());
    bytes.extend_from_slice(result);
    bytes
}

#[test]
fn reply_is_bound_to_submitted_owner_and_is_delivered_only_once() {
    let owner = owner();
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (id, request) = tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER,
        &registration(), &[RpcProperty { key: b"id", value: b"#SipGateway" }]).unwrap();
    let decoded = decode_request(request.expose_to_transport()).unwrap();
    assert_eq!(decoded.call_id, id);
    assert_eq!(decoded.method, UPDATE_STATUS_METHOD.as_bytes());
    assert_eq!(&decoded.arguments[..5], &[0, 0, 1, 0, 0]);
    let bytes = reply(id, 0, &[0, 1, 0, 0, 0, 0]);
    let callback = tracker.on_reply(&connection, &bytes).unwrap().unwrap();
    assert_eq!(callback.owner, owner);
    assert_eq!(callback.callback, Callback::HostingAck { success: true });
    assert!(tracker.on_reply(&connection, &bytes).unwrap().is_none());
}

#[test]
fn unregister_reply_never_confirms_a_later_register_request() {
    let owner = owner();
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (unregister, _) = tracker.prepare_update_status(owner.clone(), "uri", STATUS_UNREGISTER,
        &registration(), &[]).unwrap();
    let (register, _) = tracker.prepare_update_status(owner, "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert_ne!(unregister, register);
    assert!(tracker.on_reply(&connection, &reply(unregister, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    assert_eq!(tracker.on_reply(&connection, &reply(register, 0, &[0, 1, 0, 0, 0, 0])).unwrap().unwrap().callback,
        Callback::HostingAck { success: true });
}

#[test]
fn unknown_retired_or_stale_connection_reply_cannot_remove_current_pending_request() {
    let owner = owner();
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (old, _) = tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    tracker.retire_request(old);
    let (current, _) = tracker.prepare_update_status(owner, "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert_ne!(old, current);
    assert!(tracker.on_reply(&connection, &reply(old, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    assert!(tracker.on_reply(&connection, &reply(999, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    let stale_connection = HostingRpcTracker::new().connection_scope();
    assert!(tracker.on_reply(&stale_connection, &[0xff]).unwrap().is_none());
    assert!(tracker.on_reply(&stale_connection, &reply(current, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    assert!(tracker.on_reply(&connection, &reply(current, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_some());
}

#[test]
fn failed_rpc_or_malformed_bool_cannot_promote_hosting() {
    for (code, result) in [(1, &[0, 1, 0, 0, 0, 0][..]), (0, &[0][..])] {
        let mut tracker = HostingRpcTracker::new();
        let connection = tracker.connection_scope();
        let (id, _) = tracker.prepare_update_status(owner(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
        let callback = tracker.on_reply(&connection, &reply(id, code, result)).unwrap().unwrap();
        assert_eq!(callback.callback, Callback::Failure(BackendFailure::HostingRejected));
        assert!(tracker.on_reply(&connection, &reply(id, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    }
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (id, _) = tracker.prepare_update_status(owner(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert_eq!(tracker.on_reply(&connection, &reply(id, 0, &[0, 0, 0, 0, 0, 0])).unwrap().unwrap().callback,
        Callback::HostingAck { success: false });
}

#[test]
fn retiring_owner_never_rebinds_to_new_session_for_the_same_line() {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    let old = session.begin_login(100).unwrap();
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (id, _) = tracker.prepare_update_status(old.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    tracker.retire_owner(&old);
    session.import(imported(), 100).unwrap();
    let current = session.begin_login(100).unwrap();
    session.on_sdk_login(&current, true);
    session.begin_hosting(&current).unwrap();
    assert!(tracker.on_reply(&connection, &reply(id, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    assert_eq!(session.phase(), Phase::HostingPending);
}

#[test]
fn malformed_envelope_does_not_consume_an_unidentified_pending_request() {
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let (id, _) = tracker.prepare_update_status(owner(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert_eq!(tracker.on_reply(&connection, &[0]).err(), Some(CodecError::Truncated));
    assert!(tracker.on_reply(&connection, &reply(id, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_some());
}

#[test]
fn id_exhaustion_and_entry_limit_fail_closed_without_reusing_request_ids() {
    let owner = owner();
    let mut tracker = HostingRpcTracker::new();
    for _ in 0..MAX_MAP_ENTRIES {
        tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    }
    assert_eq!(tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration(), &[]).err(),
        Some(CodecError::TooLarge));
    tracker.retire_owner(&owner);
    tracker.next_id = Some(i32::MAX);
    let (last, _) = tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert_eq!(last, i32::MAX);
    tracker.retire_request(last);
    assert_eq!(tracker.prepare_update_status(owner, "uri", STATUS_REGISTER, &registration(), &[]).err(),
        Some(CodecError::InvalidCallId));
}
