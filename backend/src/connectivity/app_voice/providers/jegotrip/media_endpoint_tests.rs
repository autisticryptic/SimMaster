//! Source-only bounded original Accept -> native Endpoint fixtures. No socket,
//! UDP carrier, audible pipeline, SDK CallChanged or backend-readiness claim.
use super::*;
use super::super::{
    arc_session::ArcPhase,
    media_test_support::{accepted_context, accepted_with_properties, assert_endpoint_request,
        endpoint_result, finish_operation, invited_ticket, profile, CONTENT, ENDPOINT, NATIVE_ID},
    rpc_envelope::{decode_request, encode_reply}, rpc_stream::{Reader, Writer},
    runtime_provision::{AccountTrustInputs, ProvisionError}, tls_engine::tests::Certificates,
};

#[test]
fn actual_original_accept_is_the_only_endpoint_content_source_and_new_sequence_is_one() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let destination = RpcConnectionScope::fresh(); let supplied = profile(&cert.root, "fixture.invalid");
    let plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 49151).unwrap();
    assert!(accepted.original_call(&ticket)); assert_eq!(accepted.native_id(), ticket.native_id());
    assert_eq!(accepted.request_id(), 3); assert_eq!(accepted.operation(), CallOperation::Accept);
    assert_eq!(plan.owner(), &owner); assert!(plan.ticket().same_call(&ticket));
    assert!(plan.scope().matches(&destination)); assert!(!plan.scope().matches(&account));
    assert_endpoint_request(plan.request().unwrap(), CONTENT, 49151);
    let connection = plan.connection().unwrap();
    assert_eq!(connection.server_reference.as_bytes(), ENDPOINT);
    assert_eq!(connection.authority, "fixture.invalid"); assert_eq!(connection.root_der_base64, cert.root);
    assert!(connection.enable_sni); assert!(connection.reference_decode_key.is_none());
    assert!(account.is_open()); assert!(ticket.is_live()); assert_eq!(rpc.phase(), ArcPhase::Authenticated);
}

#[test]
fn different_original_owner_ticket_identity_and_full_native_id_cannot_reuse_accept() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (mut rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let (_other_rpc, other_owner, other_account, other_ticket, other_reply) = accepted_context(ENDPOINT, CONTENT);
    assert_ne!(owner, other_owner); assert!(!account.matches(&other_account));
    assert_eq!(ticket.native_id(), other_ticket.native_id()); assert!(!ticket.same_call(&other_ticket));
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &other_ticket, RpcConnectionScope::fresh(), supplied.clone(), 31001).err(), Some(MediaError::Scope));
    assert_eq!(MediaEndpointPlan::from_accept(&other_reply, &ticket, RpcConnectionScope::fresh(), supplied.clone(), 31001).err(), Some(MediaError::Scope));
    let different_id = invited_ticket(&mut rpc, &owner, &account, NATIVE_ID ^ (1u64 << 63));
    assert_eq!(different_id.owner(), &owner); assert!(different_id.connection_scope().matches(&account));
    assert_ne!(different_id.native_id(), ticket.native_id());
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &different_id, RpcConnectionScope::fresh(), supplied.clone(), 31001).err(), Some(MediaError::Scope));
    let (inform, _) = rpc.prepare_call(&different_id, CallOperation::Inform, &[]).unwrap();
    let inform_reply = finish_operation(&mut rpc, &owner, &account, inform, CallOperation::Inform, &[]);
    assert_eq!(MediaEndpointPlan::from_accept(&inform_reply, &different_id, RpcConnectionScope::fresh(), supplied.clone(), 31001).err(), Some(MediaError::Scope));
    let (accept, _) = rpc.prepare_call(&different_id, CallOperation::Accept, &[]).unwrap();
    let mut rejected = Writer::new(); rejected.boolean(false).unwrap();
    rejected.properties(&[RpcProperty { key: b"endpoint", value: ENDPOINT }, RpcProperty { key: b"content", value: CONTENT }]).unwrap();
    rejected.bytes(b"").unwrap(); let rejected = rejected.finish();
    let wire = encode_reply(accept, 0, rejected.expose_to_transport()).unwrap();
    let Some(super::super::arc_session::ArcReply::Call(rejected)) = rpc.on_reply(&owner, &account, wire.expose_to_transport()).unwrap()
        else { panic!("original rejected Accept required") };
    assert!(!rejected.accepted());
    assert_eq!(MediaEndpointPlan::from_accept(&rejected, &different_id, RpcConnectionScope::fresh(), supplied, 31001).err(), Some(MediaError::Scope));
    assert!(ticket.is_live());
}

#[test]
fn account_scope_closed_source_closed_destination_and_zero_bound_port_fail_before_capture() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (_rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &ticket, account.clone(), supplied.clone(), 31002).err(), Some(MediaError::Scope));
    let destination = RpcConnectionScope::fresh(); destination.close();
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 31002).err(), Some(MediaError::Scope));
    let live_destination = RpcConnectionScope::fresh();
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &ticket, live_destination.clone(), supplied.clone(), 0).err(), Some(MediaError::Scope));
    assert!(live_destination.is_open()); assert!(account.is_open());
    account.close(); assert!(!ticket.is_live()); assert!(accepted.properties_for(&owner, &account).is_none());
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &ticket, live_destination.clone(), supplied, 31002).err(), Some(MediaError::Scope));
    assert!(live_destination.is_open(), "planning does not manufacture runtime retirement");
}

#[test]
fn missing_endpoint_content_and_scoped_opaque_reference_are_not_guessed_from_other_data() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    for properties in [vec![RpcProperty { key: b"content", value: CONTENT }],
        vec![RpcProperty { key: b"endpoint", value: ENDPOINT }],
        vec![RpcProperty { key: b"RseEx.SDP", value: ENDPOINT }, RpcProperty { key: b"NativeEndpointPeer", value: CONTENT }]] {
        let (_rpc, _, account, ticket, reply) = accepted_with_properties(&properties);
        assert_eq!(MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), supplied.clone(), 31003).err(), Some(MediaError::MissingProperty));
        assert!(account.is_open());
    }
    for endpoint in [b"scoped-opaque-reference\0\xff".as_slice(), b"ENC(unresolved)", b""] {
        let (_rpc, _, _, ticket, reply) = accepted_context(endpoint, CONTENT);
        assert_eq!(MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), supplied.clone(), 31003).err(), Some(MediaError::UnsupportedReference));
    }
    let (_rpc, _, _, ticket, reply) = accepted_context(ENDPOINT, b"");
    assert_eq!(MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), supplied, 31003).err(), Some(MediaError::Configuration));
}

#[test]
fn reference_identity_authority_adapter_locator_and_root_policy_are_strict() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    for endpoint in [b"#other-endpoint:tcp -h fixture.invalid -p 443;".as_slice(),
        b"#fixture-endpoint:tcp -h other.invalid -p 443;", b"#fixture-endpoint@adapter$locator",
        b"#fixture-endpoint@adapter", b"#fixture-endpoint:udp -h fixture.invalid -p 443;",
        b"#fixture-endpoint:tls -h fixture.invalid -p 443;", b"#fixture-endpoint:tcp -h fixture.invalid;"] {
        let (_rpc, _, account, ticket, reply) = accepted_context(endpoint, CONTENT);
        assert_eq!(MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), supplied.clone(), 31004).err(), Some(MediaError::UnsupportedReference));
        assert!(account.is_open()); assert!(ticket.is_live());
    }
    let (_rpc, _, _, ticket, reply) = accepted_context(ENDPOINT, CONTENT);
    assert_eq!(MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), profile(&cert.root, "other.invalid"), 31004).err(), Some(MediaError::UnsupportedReference));
    // Syntactically invalid roots fail import. A well-formed *untrusted* root
    // cannot be detected by planning; the isolated TLS runtime tests cover it.
    for root in ["", "not-base64", "AA=="] {
        assert_eq!(AccountTrust::supplied(AccountTrustInputs { authority: "fixture.invalid", root_der_base64: root, enable_sni: true }).err(), Some(ProvisionError::Configuration));
    }
}

fn profile_inputs<'a>(trust: Arc<AccountTrust>, routing: &'a [RpcProperty<'a>], properties: &'a [RpcProperty<'a>]) -> MediaEndpointProfileInputs<'a> {
    MediaEndpointProfileInputs { trust, routing, features: &[b"zz", b"a", b"tcp"], properties,
        reference_decode_key: None, handshake_timeout: Duration::from_secs(2), partial_timeout: Duration::from_secs(2),
        io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_secs(2) }
}

#[test]
fn profile_refuses_secret_overrides_and_request_set_is_be32_length_first_without_inference() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let trust = Arc::new(AccountTrust::supplied(AccountTrustInputs { authority: "fixture.invalid", root_der_base64: &cert.root, enable_sni: true }).unwrap());
    let routing = [RpcProperty { key: b"id", value: b"#fixture-endpoint" }, RpcProperty { key: b"domain", value: b"fixture-domain" }];
    for key in [b"pwd".as_slice(), b"token", b"Client.ArcPort"] {
        let properties = [RpcProperty { key, value: b"foreign" }];
        assert_eq!(MediaEndpointProfile::supplied(profile_inputs(trust.clone(), &routing, &properties)).err(), Some(MediaError::Configuration));
    }
    let mut invalid = profile_inputs(trust.clone(), &routing, &[]); invalid.rpc_timeout = Duration::ZERO;
    assert_eq!(MediaEndpointProfile::supplied(invalid).err(), Some(MediaError::Configuration));
    let supplied = Arc::new(MediaEndpointProfile::supplied(profile_inputs(trust, &routing, &[])).unwrap());
    let (_rpc, _, _, ticket, reply) = accepted_context(ENDPOINT, CONTENT);
    let plan = MediaEndpointPlan::from_accept(&reply, &ticket, RpcConnectionScope::fresh(), supplied, 65535).unwrap();
    let request = decode_request(plan.request().unwrap()).unwrap();
    assert_eq!(request.metadata.len(), 2); assert!(!request.metadata.iter().any(|p| matches!(p.key, b"pwd" | b"token")));
    let mut body = Reader::versioned(request.arguments).unwrap(); assert_eq!(body.bytes().unwrap(), CONTENT);
    assert_eq!(body.u32().unwrap(), 3); assert_eq!(body.bytes().unwrap(), b"a");
    assert_eq!(body.bytes().unwrap(), b"zz"); assert_eq!(body.bytes().unwrap(), b"tcp");
    let parameters = body.properties().unwrap(); body.finish().unwrap();
    assert_eq!(parameters, vec![RpcProperty { key: b"Client.ArcPort", value: b"65535" }]);
    assert_eq!(plan.connection().unwrap().root_der_base64, cert.root);
}

#[test]
fn wrong_owner_scope_sequence_leave_original_pending_and_true_returns_private_correlated_data() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let (_other_rpc, other_owner, _, other_ticket, _) = accepted_context(ENDPOINT, CONTENT);
    let destination = RpcConnectionScope::fresh();
    let mut plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 31005).unwrap();
    let result = endpoint_result(true, b""); let wire = encode_reply(1, 0, result.expose_to_transport()).unwrap();
    assert!(plan.on_reply(&other_owner, &destination, wire.expose_to_transport()).unwrap().is_none());
    assert!(plan.on_reply(&owner, &account, wire.expose_to_transport()).unwrap().is_none());
    assert!(plan.on_reply(&owner, &RpcConnectionScope::fresh(), wire.expose_to_transport()).unwrap().is_none());
    let wrong = encode_reply(2, 0, result.expose_to_transport()).unwrap();
    assert!(plan.on_reply(&owner, &destination, wrong.expose_to_transport()).unwrap().is_none());
    assert_endpoint_request(plan.request().unwrap(), CONTENT, 31005);
    let proof = plan.on_reply(&owner, &destination, wire.expose_to_transport()).unwrap().unwrap();
    assert!(plan.request().is_err()); assert!(proof.matches(&ticket, &destination, &supplied));
    assert!(proof.properties(&other_ticket, &destination, &supplied).is_none());
    assert!(proof.properties(&ticket, &account, &supplied).is_none());
    assert!(proof.features(&ticket, &destination, &profile(&cert.root, "fixture.invalid")).is_none());
    assert_eq!(proof.features(&ticket, &destination, &supplied).unwrap(), vec![b"tcp".as_slice(), b"udp"]);
    let properties = proof.properties(&ticket, &destination, &supplied).unwrap();
    assert_eq!(properties.iter().find(|p| p.key == b"content").unwrap().value, b"result\0\xff");
    assert!(!properties.iter().any(|p| p.key == b"fixture-footer"));
    // Client.Host/ArcPort remain DATA. This type has no UDP/SDK callback proof.
    assert_eq!(rpc.phase(), ArcPhase::Authenticated); assert!(account.is_open()); assert!(ticket.is_live());
}

#[test]
fn duplicate_rejected_and_malformed_results_never_auto_true_retry_or_reopen_scopes() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let good = endpoint_result(true, b""); let rejected = endpoint_result(false, b""); let footer = endpoint_result(true, b"fixture-rejection");
    let mut missing_footer = Writer::new(); missing_footer.boolean(true).unwrap();
    missing_footer.u32(0).unwrap(); missing_footer.properties(&[]).unwrap(); let missing_footer = missing_footer.finish();
    for (code, bytes, expected) in [(0, rejected.expose_to_transport(), MediaError::Rejected),
        (7, good.expose_to_transport(), MediaError::Rejected), (0, footer.expose_to_transport(), MediaError::Rejected),
        (0, missing_footer.expose_to_transport(), MediaError::Protocol), (0, b"\0\x01".as_slice(), MediaError::Protocol)] {
        let (_rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
        let destination = RpcConnectionScope::fresh();
        let mut plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 31006).unwrap();
        let wire = encode_reply(1, code, bytes).unwrap();
        assert_eq!(plan.on_reply(&owner, &destination, wire.expose_to_transport()).err(), Some(expected));
        assert!(plan.request().is_err());
        let late = encode_reply(1, 0, good.expose_to_transport()).unwrap();
        assert!(plan.on_reply(&owner, &destination, late.expose_to_transport()).unwrap().is_none());
        assert!(destination.is_open(), "runtime, not plan, performs terminal cleanup");
        assert!(account.is_open()); assert!(ticket.is_live());
    }
    let (_rpc, owner, _, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let destination = RpcConnectionScope::fresh();
    let mut plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied, 31006).unwrap();
    let wire = encode_reply(1, 0, good.expose_to_transport()).unwrap();
    assert!(plan.on_reply(&owner, &destination, wire.expose_to_transport()).unwrap().is_some());
    assert!(plan.on_reply(&owner, &destination, wire.expose_to_transport()).unwrap().is_none());
}

#[test]
fn original_ticket_revoke_invalidates_captured_and_borrowed_reply_media_access() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (_rpc, owner, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let copied_ticket = ticket.captured_copy(); let destination = RpcConnectionScope::fresh();
    let original_accept_borrow = accepted.properties_for(&owner, &account).unwrap();
    let mut plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 31007).unwrap();
    let bytes = endpoint_result(true, b""); let wire = encode_reply(1, 0, bytes.expose_to_transport()).unwrap();
    let proof = plan.on_reply(&owner, &destination, wire.expose_to_transport()).unwrap().unwrap();
    let borrowed_data = proof.properties(&ticket, &destination, &supplied).unwrap();
    assert!(ticket.with_media_gate(|| borrowed_data.len()).is_some());
    copied_ticket.revoke();
    assert!(!ticket.is_live());
    assert_eq!(ticket.with_media_gate(|| (borrowed_data.len(), original_accept_borrow.len())), None);
    assert!(proof.properties(&copied_ticket, &destination, &supplied).is_none());
    assert!(proof.features(&ticket, &destination, &supplied).is_none());
    assert!(!plan.live()); assert_eq!(plan.connection().err(), Some(MediaError::Scope));
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &copied_ticket, RpcConnectionScope::fresh(), supplied, 31007).err(), Some(MediaError::Scope));
    // Existing byte borrows are snapshots; no assertion of physical erasure.
    assert!(account.is_open()); assert!(destination.is_open());
}

#[test]
fn debug_redacts_original_reference_binary_content_root_native_id_and_endpoint_result() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (_rpc, owner, _, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let destination = RpcConnectionScope::fresh();
    let mut plan = MediaEndpointPlan::from_accept(&accepted, &ticket, destination.clone(), supplied.clone(), 31008).unwrap();
    assert_eq!(format!("{plan:?}"), "MediaEndpointPlan(<redacted>)");
    assert_eq!(format!("{:?}", plan.connection().unwrap()), "ConnectionInput(<redacted>)");
    let result = endpoint_result(true, b""); let wire = encode_reply(1, 0, result.expose_to_transport()).unwrap();
    let proof = plan.on_reply(&owner, &destination, wire.expose_to_transport()).unwrap().unwrap();
    assert_eq!(format!("{proof:?}"), "OwnedMediaEndpointReply(<redacted>)");
    assert_eq!(format!("{supplied:?}"), "MediaEndpointProfile(<redacted>)");
    for rendered in [format!("{accepted:?}"), format!("{ticket:?}"), format!("{proof:?}"), format!("{result:?}")] {
        assert!(!rendered.contains("fixture")); assert!(!rendered.contains(&cert.root));
        assert!(!rendered.contains("0123456789")); assert!(!rendered.contains("65000"));
    }
}

#[test]
fn original_accept_media_admission_is_once_even_after_plan_drop_or_configuration_retry() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let supplied = profile(&cert.root, "fixture.invalid");
    let (_rpc, _, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    // Invalid local configuration does not consume the original admission.
    assert_eq!(MediaEndpointPlan::prepare(&accepted, &ticket, supplied.clone(), 0).err(), Some(MediaError::Scope));
    let plan = MediaEndpointPlan::prepare(&accepted, &ticket, supplied.clone(), 31011).unwrap();
    let destination = plan.scope();
    let competing = RpcConnectionScope::fresh();
    assert_eq!(MediaEndpointPlan::from_accept(&accepted, &ticket, competing.clone(), supplied.clone(), 31011).err(), Some(MediaError::AlreadyStarted));
    assert!(competing.is_open()); assert!(!competing.matches(&destination));
    drop(plan); assert!(!destination.is_open());
    assert_eq!(MediaEndpointPlan::prepare(&accepted, &ticket, supplied, 31011).err(), Some(MediaError::AlreadyStarted));
    assert!(account.is_open()); assert!(ticket.is_live());
}

#[test]
fn production_plan_mints_distinct_physical_scopes_for_original_calls_with_same_numeric_id() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let supplied = profile(&cert.root, "fixture.invalid");
    let (_first_rpc, _, first_account, first_ticket, first_reply) = accepted_context(ENDPOINT, CONTENT);
    let (_second_rpc, _, second_account, second_ticket, second_reply) = accepted_context(ENDPOINT, CONTENT);
    let first = MediaEndpointPlan::prepare(&first_reply, &first_ticket, supplied.clone(), 31012).unwrap();
    let second = MediaEndpointPlan::prepare(&second_reply, &second_ticket, supplied, 31013).unwrap();
    assert_eq!(first_ticket.native_id(), second_ticket.native_id());
    assert!(!first_ticket.same_call(&second_ticket)); assert!(!first.scope().matches(&second.scope()));
    assert!(!first.scope().matches(&first_account)); assert!(!second.scope().matches(&second_account));
    assert_endpoint_request(first.request().unwrap(), CONTENT, 31012);
    assert_endpoint_request(second.request().unwrap(), CONTENT, 31013);
    let second_scope = second.scope(); drop(first); assert!(second_scope.is_open());
    assert!(first_account.is_open()); assert!(second_account.is_open());
}
#[test]
fn original_mpath_fallback_omits_logical_arc_port_without_substituting_content_or_adding_a_second_owner() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let supplied = profile(&cert.root, "fixture.invalid");
    let (_rpc, _, account, ticket, accepted) = accepted_context(ENDPOINT, CONTENT);
    let plan = MediaEndpointPlan::prepare_mpath(&accepted, &ticket, supplied.clone()).unwrap();
    assert!(plan.is_mpath()); assert!(!plan.scope().matches(&account));
    let request = decode_request(plan.request().unwrap()).unwrap(); assert_eq!(request.call_id, 1);
    assert_eq!(request.method, b"connect.Endpoint.Endpoint");
    let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), CONTENT);
    let count = reader.u32().unwrap(); for _ in 0..count { reader.bytes().unwrap(); }
    let properties = reader.properties().unwrap(); reader.finish().unwrap();
    assert!(!properties.iter().any(|property| property.key == b"Client.ArcPort"));
    assert_eq!(MediaEndpointPlan::prepare(&accepted, &ticket, supplied, 49151).err(), Some(MediaError::AlreadyStarted));
    assert!(account.is_open()); assert!(ticket.is_live());
}
