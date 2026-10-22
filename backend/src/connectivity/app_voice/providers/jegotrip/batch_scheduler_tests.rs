use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_session::{ArcLoginInputs, ArcReply, ArcRpcSession},
    rpc_envelope::{encode_reply, decode_request}, rpc_stream::Writer,
    session::{HostingSession, GatewayRegistration, STATUS_REGISTER}, tests::imported,
};

fn owner() -> AttemptOwner {
    let mut s = HostingSession::new("line-a".into()); s.import(imported(), 100).unwrap(); s.begin_login(100).unwrap()
}

fn setup() -> (ArcRpcSession, BatchScheduler, super::super::rpc_pending::HostingRpcTracker, AttemptOwner) {
    let owner = owner(); let mut rpc = ArcRpcSession::new(owner.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selector", phone_login_token: b"fixture-pwd", application_key: b"fixture-key",
        domain_id: b"fixture-domain", application_id: b"fixture-app", device_info_cc: b"", client_properties: &[] }, &[]).unwrap();
    let mut result = Writer::new(); result.boolean(true).unwrap(); result.bytes(b"fixture-user").unwrap();
    result.bytes(b"fixture-token").unwrap(); result.properties(&[]).unwrap(); result.bytes(b"").unwrap();
    let result = result.finish(); let envelope = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(&owner, &scope, envelope.expose_to_transport()).unwrap(), Some(ArcReply::Login(_))));
    let tracker = rpc.take_hosting_tracker().unwrap();
    let scheduler = rpc.take_batch_scheduler(7).unwrap();
    (rpc, scheduler, tracker, owner)
}

fn registration() -> GatewayRegistration<'static> {
    GatewayRegistration { authname_prefix: "fixture-prefix", channel_id: "fixture-channel", delete_int_code: "1",
        sip_number: "fixture-number", sip_password: "fixture-secret" }
}

fn submit(rpc: &mut ArcRpcSession, scheduler: &mut BatchScheduler, now: u64) -> (i32, BatchWords) {
    let batch = scheduler.prepare(now).unwrap(); let words = batch.words();
    let (id, bytes) = rpc.prepare_scheduled_swap(&batch, &[]).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    let mut reader = super::super::rpc_stream::Reader::versioned(request.arguments).unwrap();
    reader.bytes().unwrap(); reader.bytes().unwrap();
    assert_eq!(super::super::rpc_batch::read_payload(&mut reader).unwrap().fields, words.fields());
    scheduler.commit_submission(batch, now).unwrap();
    (id, words)
}

fn response(id: i32, words: BatchWords, requests: &[(i32, &[u8], &[u8])], results: &[(i32, u32, &[u8])]) -> super::super::rpc_stream::RpcBytes {
    let mut w = Writer::new(); w.boolean(true).unwrap(); for value in words.fields() { w.u32(value).unwrap(); }
    w.u32(requests.len() as u32).unwrap();
    for &(sequence, method, args) in requests { w.i32(sequence).unwrap(); w.bytes(method).unwrap(); w.bytes(args).unwrap(); w.properties(&[]).unwrap(); }
    w.u32(results.len() as u32).unwrap();
    for &(sequence, code, bytes) in results { w.i32(sequence).unwrap(); w.u32(code).unwrap(); w.bytes(bytes).unwrap(); }
    w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); let w = w.finish();
    encode_reply(id, 0, w.expose_to_transport()).unwrap()
}

fn batch_reply<'a>(rpc: &mut ArcRpcSession, owner: &AttemptOwner, bytes: &'a [u8]) -> OwnedBatchReply<'a> {
    let scope = rpc.connection_scope();
    match rpc.on_reply(owner, &scope, bytes).unwrap().unwrap() { ArcReply::Swap(reply) => reply, _ => panic!("batch expected") }
}

fn peer() -> BatchWords { BatchWords { incarnation: 9, ..BatchWords::default() } }

#[test]
fn batch_words_independently_match_native_exclusive_index_masks_and_empty_poll() {
    let (mut rpc, mut scheduler, mut tracker, owner) = setup();
    let (_, initial) = submit(&mut rpc, &mut scheduler, 0);
    assert_eq!(initial.fields(), [7, 0, 0, 0, 0, 0]); assert_eq!(scheduler.history_count(), 0);
    let call = tracker.prepare_batch_update_status(owner, "fixture-uri", STATUS_REGISTER, &registration()).unwrap();
    scheduler.queue_hosting(&call, &[]).unwrap();
    let (_, first) = submit(&mut rpc, &mut scheduler, 1);
    assert_eq!(first.fields(), [7, 1, 1, 0, 0, 0]);
    let (_, retained) = submit(&mut rpc, &mut scheduler, 2);
    assert_eq!(retained.fields(), [7, 1, 1, 0, 0, 0]); assert_eq!(scheduler.history_count(), 1);
}

#[test]
fn ack_bit_erases_exact_epoch_but_does_not_acknowledge_hosting() {
    let (mut rpc, mut scheduler, mut tracker, owner) = setup();
    for now in 0..2 {
        let call = tracker.prepare_batch_update_status(owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
        scheduler.queue_hosting(&call, &[]).unwrap(); submit(&mut rpc, &mut scheduler, now);
    }
    let (id, words) = submit(&mut rpc, &mut scheduler, 2); assert_eq!(words.batches, 3);
    let envelope = response(id, BatchWords { ack_incarnation: 7, ack_next_batch: 2, ack_batches: 2, ..peer() }, &[], &[]);
    let reply = batch_reply(&mut rpc, &owner, envelope.expose_to_transport());
    let delivery = scheduler.on_reply(&reply, 2).unwrap().unwrap();
    assert!(delivery.matched_result_sequences.is_empty());
    assert!(tracker.on_scheduled_delivery(&delivery).is_empty());
    assert_eq!(scheduler.history_count(), 1); assert_eq!(scheduler.pending_client_count(), 2);
    let (_, words) = submit(&mut rpc, &mut scheduler, 3);
    assert_eq!(words.fields(), [7, 2, 1, 9, 0, 0]);
}

#[test]
fn retained_retransmit_keeps_original_method_arguments_metadata_and_sequence() {
    let (mut rpc, mut scheduler, mut tracker, owner) = setup();
    let call = tracker.prepare_batch_update_status(owner, "fixture-uri", STATUS_REGISTER, &registration()).unwrap();
    let metadata = [super::super::rpc_stream::RpcProperty { key: b"__magic", value: &[0, 0xff] }];
    scheduler.queue_hosting(&call, &metadata).unwrap();
    submit(&mut rpc, &mut scheduler, 10);
    let batch = scheduler.prepare(11).unwrap(); let payload = batch.payload(); let request = &payload.requests[&call.sequence];
    assert_eq!(request.method, super::super::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
    assert_eq!(request.arguments, call.as_batch_request(&[]).arguments); assert_eq!(request.metadata, metadata);
    assert!(!format!("{batch:?}").contains("fixture"));
    assert_eq!(scheduler.queue_hosting(&call, &[]).err(), Some(BatchError::Epoch));
}

#[test]
fn negative_old_and_duplicate_sequences_never_reset_the_dedup_window() {
    let mut window = DedupWindow::default();
    assert!(window.insert(1)); assert!(!window.insert(1)); assert!(window.insert(2047)); assert!(window.insert(2048));
    assert_eq!(window.base, 32); assert_eq!(window.head, 1); assert!(!window.insert(1)); assert!(!window.insert(2048));
    assert!(window.insert(4095)); assert_eq!(window.base, 2048); assert_eq!(window.head, 0);
    assert!(!window.insert(2048)); assert!(window.insert(8192)); assert_eq!(window.base, 6176);
    assert!(!window.insert(4095)); assert!(!window.insert(8192));
}

#[test]
fn peer_ack_window_merges_newer_and_older_masks_without_shifts_by_32() {
    let mut next = 0; let mut mask = 0;
    merge_bitmap(&mut next, &mut mask, 1, 1); assert_eq!((next, mask), (1, 1));
    merge_bitmap(&mut next, &mut mask, 3, 1); assert_eq!((next, mask), (3, 5));
    merge_bitmap(&mut next, &mut mask, 2, 1); assert_eq!((next, mask), (3, 7));
    merge_bitmap(&mut next, &mut mask, 35, 1); assert_eq!((next, mask), (35, 1));
    merge_bitmap(&mut next, &mut mask, 3, 7); assert_eq!((next, mask), (35, 1));
    assert!(!valid_bitmap(0, 1)); assert!(!valid_bitmap(2, 4)); assert!(valid_bitmap(32, u32::MAX));
}

#[test]
fn server_request_ticket_survives_one_reply_and_queues_original_sequence_result() {
    let (mut rpc, mut scheduler, _, owner) = setup(); let (id, _) = submit(&mut rpc, &mut scheduler, 0);
    let envelope = response(id, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[(5, b"fixture-method", b"args")], &[]);
    let reply = batch_reply(&mut rpc, &owner, envelope.expose_to_transport());
    let mut delivered = scheduler.on_reply(&reply, 0).unwrap().unwrap(); assert_eq!(delivered.requests.len(), 1);
    let incoming = delivered.requests.remove(0); assert_eq!(incoming.ticket.sequence(), 5);
    assert_eq!(incoming.request().arguments, b"args");
    assert!(scheduler.complete_server_request(&incoming.ticket, 0, &[0, 1, 0, 0, 0, 0]).unwrap());
    assert!(!scheduler.complete_server_request(&incoming.ticket, 0, &[0, 1, 0, 0, 0, 0]).unwrap());
    let batch = scheduler.prepare(1).unwrap();
    assert_eq!(batch.words().fields(), [7, 1, 1, 9, 1, 1]);
    assert_eq!(batch.payload().results[&5].result_stream, &[0, 1, 0, 0, 0, 0]);
}

#[test]
fn duplicate_server_request_on_second_owned_reply_is_not_redelivered() {
    let (mut rpc, mut scheduler, _, owner) = setup();
    for now in 0..2 {
        let (id, _) = submit(&mut rpc, &mut scheduler, now);
        let envelope = response(id, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[(1, b"method", b"args")], &[]);
        let reply = batch_reply(&mut rpc, &owner, envelope.expose_to_transport());
        assert_eq!(scheduler.on_reply(&reply, now).unwrap().unwrap().requests.len(), usize::from(now == 0));
    }
}

#[test]
fn peer_incarnation_change_wrong_ack_future_mask_or_nonpositive_sequence_fail_closed() {
    for variant in 0..5 {
        let (mut rpc, mut scheduler, _, owner) = setup(); let (id, _) = submit(&mut rpc, &mut scheduler, 0);
        let envelope = response(id, peer(), &[], &[]); let reply = batch_reply(&mut rpc, &owner, envelope.expose_to_transport());
        scheduler.on_reply(&reply, 0).unwrap();
        let (id, _) = submit(&mut rpc, &mut scheduler, 1);
        let mut words = peer(); let mut requests = Vec::new();
        match variant {
            0 => words.incarnation = 10, 1 => words.ack_incarnation = 8,
            2 => { words.ack_incarnation = 7; words.ack_next_batch = 1; words.ack_batches = 1; },
            3 => { words.next_batch = 1; words.batches = 2; },
            _ => requests.push((0, b"method".as_slice(), b"args".as_slice())),
        }
        let envelope = response(id, words, &requests, &[]); let reply = batch_reply(&mut rpc, &owner, envelope.expose_to_transport());
        assert_eq!(scheduler.on_reply(&reply, 1).err(), Some(BatchError::Epoch)); assert!(!scheduler.is_open());
        assert_eq!(rpc.phase(), super::super::arc_session::ArcPhase::Closed);
    }
}

#[test]
fn unacknowledged_retention_and_33rd_epoch_are_explicit_failure_not_silent_success() {
    for expire in [true, false] {
        let original = owner(); let scope = RpcConnectionScope::fresh();
        let mut scheduler = BatchScheduler::for_arc_session(original.clone(), scope.clone(), 7).unwrap();
        let mut tracker = super::super::rpc_pending::HostingRpcTracker::for_arc_session(original, scope.clone());
        let count = if expire { 1 } else { MAX_BATCH_EPOCHS };
        for now in 0..count {
            let call = tracker.prepare_batch_update_status(scheduler.owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
            scheduler.queue_hosting(&call, &[]).unwrap();
            let batch = scheduler.prepare(now as u64).unwrap(); batch.mark_encoded();
            scheduler.commit_submission(batch, now as u64).unwrap();
        }
        if !expire { let call = tracker.prepare_batch_update_status(scheduler.owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap(); scheduler.queue_hosting(&call, &[]).unwrap(); }
        let now = if expire { BATCH_RETENTION_MS } else { count as u64 };
        assert_eq!(scheduler.prepare(now).err(), Some(BatchError::Retention)); assert!(!scheduler.is_open());
        assert!(!scope.is_open());
    }
}

#[test]
fn malformed_or_false_outer_batch_invalidates_all_pending_scheduler_scope() {
    let (mut rpc, mut scheduler, _, owner) = setup(); let (id, _) = submit(&mut rpc, &mut scheduler, 0);
    let scope = rpc.connection_scope(); let envelope = encode_reply(id, 0, &[0, 0]).unwrap();
    assert!(matches!(rpc.on_reply(&owner, &scope, envelope.expose_to_transport()).unwrap(), Some(ArcReply::Failure(_))));
    assert_eq!(scheduler.prepare(1).err(), Some(BatchError::Closed));
}

#[test]
fn original_prepared_call_or_scheduler_scope_cannot_be_rebound_to_another_connection() {
    let (mut rpc, mut scheduler, mut tracker, owner) = setup();
    let (mut other_rpc, mut other_scheduler, _, other_owner) = setup();
    let mut call = tracker.prepare_batch_update_status(owner, "uri", STATUS_REGISTER, &registration()).unwrap();
    assert_eq!(other_scheduler.queue_hosting(&call, &[]).err(), Some(BatchError::Scope));
    let original_seq = call.sequence; call.sequence += 1;
    assert_eq!(scheduler.queue_hosting(&call, &[]).err(), Some(BatchError::Scope)); call.sequence = original_seq;
    scheduler.queue_hosting(&call, &[]).unwrap();
    let batch = scheduler.prepare(0).unwrap();
    assert!(other_rpc.prepare_scheduled_swap(&batch, &[]).is_err());
    let (id, _) = submit(&mut other_rpc, &mut other_scheduler, 0);
    let envelope = response(id, peer(), &[], &[]); let reply = batch_reply(&mut other_rpc, &other_owner, envelope.expose_to_transport());
    assert!(scheduler.on_reply(&reply, 0).unwrap().is_none());
    rpc.close(); assert!(!scheduler.is_open());
}

#[test]
fn server_ticket_backpressure_keeps_original_ticket_for_retry_without_duplicate_completion() {
    let (mut rpc, mut scheduler, _, original) = setup(); let (id, _) = submit(&mut rpc, &mut scheduler, 0);
    let envelope = response(id, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[(1, b"method", b"args")], &[]);
    let reply = batch_reply(&mut rpc, &original, envelope.expose_to_transport());
    let delivery = scheduler.on_reply(&reply, 0).unwrap().unwrap(); let ticket = delivery.requests[0].ticket();
    let batch = scheduler.prepare(1).unwrap();
    assert_eq!(scheduler.complete_server_request(ticket, 0, &[0, 1, 0, 0, 0, 0]).err(), Some(BatchError::Backpressure));
    rpc.prepare_scheduled_swap(&batch, &[]).unwrap(); scheduler.commit_submission(batch, 1).unwrap();
    assert!(scheduler.complete_server_request(ticket, 0, &[0, 1, 0, 0, 0, 0]).unwrap());
    assert!(!scheduler.complete_server_request(ticket, 0, &[0, 1, 0, 0, 0, 0]).unwrap());
}

#[test]
fn retained_request_size_and_positive_sequence_exhaustion_are_bounded_before_submission() {
    let original = owner(); let scope = RpcConnectionScope::fresh();
    let mut scheduler = BatchScheduler::for_arc_session(original.clone(), scope.clone(), 7).unwrap();
    let mut tracker = super::super::rpc_pending::HostingRpcTracker::for_arc_session(original, scope);
    let call = tracker.prepare_batch_update_status(scheduler.owner.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    let big = vec![0; MAX_FIELD_BYTES];
    let metadata = [super::super::rpc_stream::RpcProperty { key: b"one", value: &big },
        super::super::rpc_stream::RpcProperty { key: b"two", value: &big }];
    assert!(scheduler.queue_hosting(&call, &metadata).is_err());
    assert_eq!(scheduler.pending_client_count(), 0);
    scheduler.queue_hosting(&call, &[]).unwrap(); scheduler.next_batch = i32::MAX as u32;
    assert_eq!(scheduler.prepare(0).err(), Some(BatchError::Epoch));
    assert!(!scheduler.is_open());
}

#[test]
fn result_before_ack_completes_only_once_and_scheduled_batch_cannot_bypass_validation() {
    let (mut rpc, mut scheduler, mut tracker, original) = setup();
    let call = tracker.prepare_batch_update_status(original.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    scheduler.queue_hosting(&call, &[]).unwrap();
    for now in 0..2 {
        let (id, _) = submit(&mut rpc, &mut scheduler, now);
        let envelope = response(id, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[], &[(call.sequence, 0, &[0, 1, 0, 0, 0, 0])]);
        let reply = batch_reply(&mut rpc, &original, envelope.expose_to_transport());
        assert_eq!(tracker.on_batch_reply(&reply).err(), Some(CodecError::InvalidField));
        let delivery = scheduler.on_reply(&reply, now).unwrap().unwrap();
        assert_eq!(delivery.matched_result_sequences.len(), usize::from(now == 0));
        assert_eq!(tracker.on_scheduled_delivery(&delivery).len(), usize::from(now == 0));
        assert_eq!(scheduler.history_count(), 1);
    }
}

#[test]
fn queued_but_not_submitted_hosting_cannot_be_completed_by_an_unrelated_poll_result() {
    let (mut rpc, mut scheduler, mut tracker, original) = setup();
    let (poll, _) = submit(&mut rpc, &mut scheduler, 0);
    let call = tracker.prepare_batch_update_status(original.clone(), "uri", STATUS_REGISTER, &registration()).unwrap();
    scheduler.queue_hosting(&call, &[]).unwrap();
    let envelope = response(poll, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[], &[(call.sequence, 0, &[0, 1, 0, 0, 0, 0])]);
    let reply = batch_reply(&mut rpc, &original, envelope.expose_to_transport());
    let delivery = scheduler.on_reply(&reply, 0).unwrap().unwrap();
    assert!(delivery.matched_result_sequences.is_empty()); assert!(tracker.on_scheduled_delivery(&delivery).is_empty());
    assert_eq!(scheduler.pending_client_count(), 1);
    let (swap, _) = submit(&mut rpc, &mut scheduler, 1);
    let envelope = response(swap, BatchWords { next_batch: 1, batches: 1, ..peer() }, &[], &[(call.sequence, 0, &[0, 1, 0, 0, 0, 0])]);
    let reply = batch_reply(&mut rpc, &original, envelope.expose_to_transport());
    let delivery = scheduler.on_reply(&reply, 1).unwrap().unwrap(); assert_eq!(tracker.on_scheduled_delivery(&delivery).len(), 1);
}

#[test]
fn cancelled_snapshot_cannot_later_encode_or_commit_and_original_work_is_not_lost() {
    let (mut rpc, mut scheduler, mut tracker, original) = setup();
    let call = tracker.prepare_batch_update_status(original, "uri", STATUS_REGISTER, &registration()).unwrap();
    scheduler.queue_hosting(&call, &[]).unwrap(); let stale = scheduler.prepare(0).unwrap();
    scheduler.cancel_prepared();
    assert!(rpc.prepare_scheduled_swap(&stale, &[]).is_err());
    assert_eq!(scheduler.commit_submission(stale, 0).err(), Some(BatchError::Scope));
    let fresh = scheduler.prepare(1).unwrap(); assert_eq!(fresh.payload().requests.len(), 1);
    assert!(rpc.prepare_scheduled_swap(&fresh, &[]).is_ok());
    assert!(rpc.prepare_scheduled_swap(&fresh, &[]).is_err()); scheduler.commit_submission(fresh, 1).unwrap();
    assert_eq!(scheduler.history_count(), 1);
    let escaped = scheduler.prepare(2).unwrap(); rpc.prepare_scheduled_swap(&escaped, &[]).unwrap();
    scheduler.cancel_prepared(); assert!(!scheduler.is_open()); assert_eq!(rpc.phase(), super::super::arc_session::ArcPhase::Closed);
}

#[test]
fn scheduler_is_taken_once_after_login_and_disables_untracked_raw_swaps() {
    let (mut rpc, mut scheduler, _, _) = setup();
    assert!(rpc.take_batch_scheduler(8).is_err());
    let empty = BatchPayload { fields: [0; 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    assert!(rpc.prepare_swap_calls(&empty, &[]).is_err());
    assert!(scheduler.prepare(10).is_ok());
    assert_eq!(scheduler.prepare(9).err(), Some(BatchError::Clock));
}
