//! Actions-only controls for the late Info stop peer. No account or socket.
use super::*;
use n::rpc_batch::{BatchPayload, BatchRequest, BatchResult, encode_swap_calls, SWAP_CALLS_METHOD};

fn packet(outer: i32, variant: u8) -> Vec<u8> {
    let mut args = Writer::versioned().unwrap();
    args.bytes(b"fixture-exact-gateway-uri").unwrap();
    args.properties(&[RpcProperty { key: b"SipFlag", value: if variant == 2 { b"4" } else { b"3" } }]).unwrap();
    let args = args.finish();
    let mut batch = BatchPayload { fields: [7, 2, 1, 9, 1, 1], requests: Default::default(), results: Default::default() };
    let nested = BatchRequest { method: n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes(), arguments: args.expose_to_transport(),
        metadata: vec![RpcProperty { key: b"id", value: if variant == 3 { b"other" } else { b"#SipGateway" } }] };
    batch.requests.insert(if variant == 1 { 8 } else { 7 }, nested);
    if variant == 4 { batch.fields[0] += 1; }
    if variant == 5 { batch.results.insert(1, BatchResult { result_code: 0, result_stream: b"result" }); }
    let outer_args = encode_swap_calls(if variant == 6 { b"wrong-user" } else { b"fixture-user" },
        if variant == 7 { b"wrong-selector" } else { b"fixture-original-selector" }, &batch).unwrap();
    let request = encode_request(outer, &[RpcProperty { key: b"token", value: if variant == 8 { b"other" } else { b"fixture-auth" } }],
        if variant == 9 { b"other-method" } else { SWAP_CALLS_METHOD.as_bytes() }, outer_args.expose_to_transport()).unwrap();
    let mut packet = vec![0]; packet.extend_from_slice(request.expose_to_transport()); packet
}

#[test]
fn info_sync_stop_fixture_accepts_only_identical_retained_request_under_a_fresh_outer_id() {
    let bytes = packet(11, 0);
    let (original, _) = stop_poll(&bytes);
    assert!(retained_status(&original, &stop_poll(&packet(12, 0)).0));
    for (id, variant) in [(11, 0), (10, 0), (12, 1), (12, 2), (12, 3), (12, 4), (12, 8)] {
        assert!(!retained_status(&original, &stop_poll(&packet(id, variant)).0));
    }
    for variant in [5, 6, 7, 9] {
        let bad = packet(12, variant);
        assert!(std::panic::catch_unwind(|| { stop_poll(&bad); }).is_err());
    }
}
