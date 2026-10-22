use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_session::{ArcLoginInputs, ArcReply, ArcRpcSession, OwnedBatchReply},
    rpc_batch::BatchPayload, rpc_envelope::decode_request, session::{HostingSession, Phase}, tests::imported,
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

fn batch_reply(sequence: i32) -> Vec<u8> {
    let mut bytes = vec![0, 1]; bytes.extend_from_slice(&[0; 24]);
    bytes.extend_from_slice(&[0; 4]); bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&sequence.to_be_bytes()); bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&6u32.to_be_bytes()); bytes.extend_from_slice(&[0, 1, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0; 8]); bytes
}

fn batch_session(owner: AttemptOwner) -> (ArcRpcSession, HostingRpcTracker) {
    let mut rpc = ArcRpcSession::new(owner.clone());
    let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(ArcLoginInputs {
        typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector",
        phone_login_token: b"fixture-phone", application_key: b"fixture-key", domain_id: b"fixture-domain",
        application_id: b"fixture-app", device_info_cc: b"", client_properties: &[],
    }, &[]).unwrap();
    let mut result = vec![0, 1];
    for value in [b"fixture-user".as_slice(), b"fixture-auth".as_slice()] {
        result.extend_from_slice(&(value.len() as u32).to_be_bytes()); result.extend_from_slice(value);
    }
    result.extend_from_slice(&[0; 8]);
    assert!(matches!(rpc.on_reply(&owner, &scope, &reply(id, 0, &result)).unwrap(), Some(ArcReply::Login(_))));
    let tracker = rpc.take_hosting_tracker().unwrap();
    (rpc, tracker)
}

fn swap_envelope(rpc: &mut ArcRpcSession, result: &[u8]) -> Vec<u8> {
    let payload = BatchPayload { fields: [0; 6], requests: std::collections::BTreeMap::new(),
        results: std::collections::BTreeMap::new() };
    let (id, _) = rpc.prepare_swap_calls(&payload, &[]).unwrap();
    reply(id, 0, result)
}

fn matched_batch<'a>(rpc: &mut ArcRpcSession, owner: &AttemptOwner, envelope: &'a [u8]) -> OwnedBatchReply<'a> {
    let scope = rpc.connection_scope();
    match rpc.on_reply(owner, &scope, envelope).unwrap().unwrap() {
        ArcReply::Swap(batch) => batch, _ => panic!("successful owned batch expected"),
    }
}

#[test]
fn nested_batch_reply_acknowledges_only_the_original_submitted_hosting_call() {
    let owner = owner(); let (mut rpc, mut tracker) = batch_session(owner.clone());
    let request = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    assert_eq!(request.as_batch_request(&[]).method, UPDATE_STATUS_METHOD.as_bytes());
    assert!(!format!("{request:?}").contains("fixture"));
    let envelope = swap_envelope(&mut rpc, &batch_reply(request.sequence));
    let batch = matched_batch(&mut rpc, &owner, &envelope);
    assert_eq!(batch.owner(), &owner);
    assert!(!format!("{batch:?}").contains("fixture"));
    let callbacks = tracker.on_batch_reply(&batch).unwrap();
    assert_eq!(callbacks.len(), 1);
    assert_eq!(callbacks[0].owner, owner);
    assert_eq!(callbacks[0].callback, Callback::HostingAck { success: true });
    assert!(tracker.on_batch_reply(&batch).unwrap().is_empty());
}

#[test]
fn nested_batch_scope_retirement_and_unregister_cannot_confirm_new_registration() {
    let owner = owner(); let (mut rpc, mut tracker) = batch_session(owner.clone());
    let unhosting = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_UNREGISTER, &registration()).unwrap();
    let current = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    let envelope = swap_envelope(&mut rpc, &batch_reply(unhosting.sequence));
    let old_reply = matched_batch(&mut rpc, &owner, &envelope);
    assert!(tracker.on_batch_reply(&old_reply).unwrap().is_empty());
    let (mut other_rpc, mut other_tracker) = batch_session(owner.clone());
    let other = other_tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    assert_eq!(other.sequence, unhosting.sequence);
    let other_envelope = swap_envelope(&mut other_rpc, &batch_reply(current.sequence));
    let other_reply = matched_batch(&mut other_rpc, &owner, &other_envelope);
    assert!(tracker.on_batch_reply(&other_reply).unwrap().is_empty());
    let envelope = swap_envelope(&mut rpc, &batch_reply(current.sequence));
    let batch = matched_batch(&mut rpc, &owner, &envelope);
    assert_eq!(tracker.on_batch_reply(&batch).unwrap().len(), 1);
}

#[test]
fn envelope_and_batch_delivery_ids_cannot_complete_one_another() {
    let owner = owner(); let (mut rpc, mut tracker) = batch_session(owner.clone()); let scope = tracker.connection_scope();
    let nested = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    let (outer, _) = tracker.prepare_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    assert!(tracker.on_reply(&scope, &reply(nested.sequence, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_none());
    let envelope = swap_envelope(&mut rpc, &batch_reply(outer));
    let batch = matched_batch(&mut rpc, &owner, &envelope);
    assert!(tracker.on_batch_reply(&batch).unwrap().is_empty());
    let envelope = swap_envelope(&mut rpc, &batch_reply(nested.sequence));
    let batch = matched_batch(&mut rpc, &owner, &envelope);
    assert_eq!(tracker.on_batch_reply(&batch).unwrap().len(), 1);
    assert!(tracker.on_reply(&scope, &reply(outer, 0, &[0, 1, 0, 0, 0, 0])).unwrap().is_some());
}

#[test]
fn coinciding_sequences_on_other_connections_or_line_incarnations_cannot_rebind_batch_ack() {
    let original = owner(); let different_incarnation = owner();
    for other_owner in [original.clone(), different_incarnation] {
        let (mut rpc, mut tracker) = batch_session(original.clone());
        let (mut other_rpc, mut other_tracker) = batch_session(other_owner.clone());
        let current = tracker.prepare_batch_update_status(original.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
        let other = other_tracker.prepare_batch_update_status(other_owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
        assert_eq!(current.sequence, other.sequence);
        let other_envelope = swap_envelope(&mut other_rpc, &batch_reply(other.sequence));
        let other_reply = matched_batch(&mut other_rpc, &other_owner, &other_envelope);
        assert!(tracker.on_batch_reply(&other_reply).unwrap().is_empty());
        assert_eq!(other_tracker.on_batch_reply(&other_reply).unwrap().len(), 1);
        let envelope = swap_envelope(&mut rpc, &batch_reply(current.sequence));
        let batch = matched_batch(&mut rpc, &original, &envelope);
        assert_eq!(tracker.on_batch_reply(&batch).unwrap().len(), 1);
    }
}

#[test]
fn terminal_arc_failure_or_drop_invalidates_pending_and_retained_owned_batch_replies() {
    for fail_with_batch in [true, false] {
        let owner = owner(); let (mut rpc, mut tracker) = batch_session(owner.clone());
        let current = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
        let envelope = swap_envelope(&mut rpc, &batch_reply(current.sequence));
        let retained = matched_batch(&mut rpc, &owner, &envelope);
        if fail_with_batch {
            let mut false_result = batch_reply(current.sequence); false_result[1] = 0;
            let false_envelope = swap_envelope(&mut rpc, &false_result); let scope = rpc.connection_scope();
            assert!(matches!(rpc.on_reply(&owner, &scope, &false_envelope).unwrap(), Some(ArcReply::Failure(_))));
        } else { drop(rpc); }
        assert!(tracker.on_batch_reply(&retained).unwrap().is_empty());
        assert!(tracker.pending.is_empty());
        assert_eq!(tracker.prepare_batch_update_status(owner, "uri", STATUS_REGISTER, &registration()).err(),
            Some(CodecError::InvalidField));
    }
}

#[test]
fn nested_tracker_is_taken_once_and_rejects_wrong_owner_or_unbound_mock_tracker() {
    let original = owner(); let (mut rpc, mut tracker) = batch_session(original.clone());
    assert!(rpc.take_hosting_tracker().is_err());
    assert_eq!(tracker.prepare_batch_update_status(owner(), "uri", STATUS_REGISTER, &registration()).err(),
        Some(CodecError::InvalidField));
    assert_eq!(HostingRpcTracker::new().prepare_batch_update_status(original.clone(), "uri", STATUS_REGISTER,
        &registration()).err(), Some(CodecError::InvalidField));
    let current = tracker.prepare_batch_update_status(original, "uri", STATUS_REGISTER, &registration()).unwrap();
    assert_eq!(current.sequence, 1);
}
