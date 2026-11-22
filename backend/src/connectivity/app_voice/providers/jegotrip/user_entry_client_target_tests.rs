//! Independent SOURCE-only test leaf for endpoint-less Client target DATA.
//! Fixtures create sealed matched bootstrap/login replies through ArcRpcSession;
//! no raw-data constructor, protocol peer, network, SDK or operation is used.
//! Not executed here. Router policy uses the existing test-only certificate
//! fixture (which would generate a public test certificate if later executed).
use super::NewClientTarget;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{
    arc_session::{ArcLoginInputs, ArcPhase, ArcReply, ArcRpcSession, OwnedBootstrapReply},
    legacy_config::{tests::fixture_envelope, ConfigDecodeKey},
    object_reference::{parse_object_reference, ReferenceTarget},
    router_connector::{RouterAnchor, RouterPolicy},
    rpc_envelope::encode_reply,
    rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, RpcProperty, Writer, MAX_FIELD_BYTES},
    session::{AttemptOwner, HostingSession},
    user_entry_reference::UserEntryReference,
};
use std::sync::OnceLock;

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-client-target-owner".into());
    session.import(n::tests::imported(), 100).unwrap();
    session.begin_login(100).unwrap()
}

fn policy() -> RouterPolicy {
    // Same public test configuration, but a distinct immutable policy identity
    // on EVERY call; value equality must never authorize a replacement policy.
    static ROOT: OnceLock<String> = OnceLock::new();
    let root = ROOT.get_or_init(|| n::tls_engine::tests::Certificates::generate("IP:127.0.0.1").root.clone());
    RouterPolicy::supplied("Router:tls -h 127.0.0.1 -p 12345 -ri 12001;", &[
        RouterAnchor { router_id: 12001, host: "127.0.0.1", port: 12345,
            authority: "127.0.0.1", root, enable_sni: false },
    ]).unwrap()
}

fn auxiliary_for(who: &AttemptOwner, reference: &[u8]) -> (ArcRpcSession, RpcConnectionScope, OwnedBootstrapReply) {
    let mut rpc = ArcRpcSession::new(who.clone());
    let source = rpc.connection_scope();
    let (id, _) = rpc.prepare_bootstrap(b"fixture-appkey", &[]).unwrap();
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    w.properties(&[
        RpcProperty { key: b"id", value: b"wrong-bootstrap-metadata-id" },
        RpcProperty { key: b"UserEntry", value: b"wrong-property-reference" },
    ]).unwrap();
    w.bytes(b"AccountEntry:tcp -h fixture.invalid -p 443;").unwrap();
    w.bytes(b"wrong-account-alive-reference").unwrap();
    w.bytes(reference).unwrap();
    w.bytes(b"").unwrap();
    let result = w.finish();
    let raw = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    let reply = match rpc.on_reply(who, &source, raw.expose_to_transport()).unwrap().unwrap() {
        ArcReply::Bootstrap(reply) => reply,
        _ => panic!("sealed matched bootstrap required"),
    };
    (rpc, source, reply)
}

fn auxiliary(reference: &[u8]) -> (ArcRpcSession, AttemptOwner, RpcConnectionScope, OwnedBootstrapReply) {
    let who = owner();
    let (rpc, source, reply) = auxiliary_for(&who, reference);
    (rpc, who, source, reply)
}

fn current_native_for(who: &AttemptOwner) -> (ArcRpcSession, RpcConnectionScope) {
    let mut rpc = ArcRpcSession::new(who.clone());
    rpc.claim_common_transport().unwrap();
    rpc.bind_common_identity(b"AccountEntry").unwrap();
    let parent = rpc.connection_scope();
    let inputs = ArcLoginInputs {
        typed_identity: b"username:fixture-user", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selected-session", phone_login_token: b"fixture-phone-token",
        application_key: b"fixture-app-key", domain_id: b"fixture-domain-id",
        application_id: b"fixture-app-id", device_info_cc: b"fixture-cc", client_properties: &[],
    };
    let (id, _) = rpc.prepare_login(inputs, &[]).unwrap();
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    w.bytes(b"fixture-returned-user").unwrap();
    w.bytes(b"fixture-returned-auth-token").unwrap();
    w.properties(&[]).unwrap();
    w.bytes(b"").unwrap();
    let result = w.finish();
    let raw = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(who, &parent, raw.expose_to_transport()).unwrap(), Some(ArcReply::Login(_))));
    (rpc, parent)
}

fn capture(reply: &OwnedBootstrapReply, who: &AttemptOwner, source: &RpcConnectionScope,
    parent: &RpcConnectionScope, router: &RouterPolicy) -> Box<NewClientTarget> {
    NewClientTarget::from_bootstrap(reply, who, source, parent, router, None,
        b"fixture-realm.invalid", b"fixture-cc").unwrap().unwrap()
}

fn assert_denied(target: &NewClientTarget, who: &AttemptOwner, parent: &RpcConnectionScope, router: &RouterPolicy) {
    assert!(!target.matches(who, parent, router));
    assert!(target.factory_input_for(who, parent, router).is_none());
    assert!(target.realm_for(who, parent, router).is_none());
    assert!(target.cc_for(who, parent, router).is_none());
}

#[test]
fn client_target_keeps_bare_whitespace_and_grouped_whole_factory_input() {
    let router = policy();
    for raw in [b"UserEntry".as_slice(), b"\t prefix/UserEntry \r\n",
        b" prefix/{facet:[a:b]}/UserEntry "] {
        let (rpc, who, source, reply) = auxiliary(raw);
        let parent = RpcConnectionScope::fresh();
        let target = capture(&reply, &who, &source, &parent, &router);
        assert_eq!(target.factory_input_for(&who, &parent, &router), Some(raw));
        assert!(matches!(target.reference.target(), ReferenceTarget::IdentityOnly));
        if raw != b"UserEntry" {
            assert_ne!(target.factory_input_for(&who, &parent, &router).unwrap(), target.reference.identity());
            assert_ne!(target.factory_input_for(&who, &parent, &router).unwrap(),
                target.reference.encode_native().unwrap().expose_to_backend());
        }
        assert_eq!(target.original_bootstrap_id(), reply.call_id());
        assert_eq!(rpc.phase(), ArcPhase::Initial);
        assert!(source.is_open());
    }
}

#[test]
fn client_target_retains_adapter_locator_as_data_without_changing_qualified_target() {
    let router = policy();
    for raw in [b"UserEntry@adapter".as_slice(), b" prefix/UserEntry@adapter$locator ",
        b" prefix/UserEntry@{[a$b]}$<locator:{c$d}> "] {
        let (rpc, who, source, reply) = auxiliary(raw);
        let parent = RpcConnectionScope::fresh();
        let target = capture(&reply, &who, &source, &parent, &router);
        assert!(matches!(target.reference.target(), ReferenceTarget::Adapter { .. }));
        assert_eq!(target.factory_input_for(&who, &parent, &router), Some(raw));
        assert!(target.reference.direct_raw_tcp().is_err());
        // This separate DATA holder does not relax the qualified-only leaf.
        assert!(n::user_entry_target::UserEntryTarget::from_bootstrap(&reply, &who, &source,
            &parent, &router, None, b"fixture-realm.invalid", b"").is_err());
        assert!(rpc.user_entry_auth().is_err());
        assert_eq!(rpc.phase(), ArcPhase::Initial);
    }
}

#[test]
fn client_target_rejects_every_explicit_endpoint_vector_without_dropping_entries() {
    let router = policy();
    for raw in [b"UserEntry:arc -h 42.7 -p 98;".as_slice(),
        b"UserEntry:sarc -h 42.7 -p 98;", b"UserEntry:arc -h arc@fixture-group -p 98;",
        b"UserEntry:varc -h 42.7 -p 98;", b"UserEntry:tcp -h fixture.invalid -p 443;",
        b"UserEntry:tls -h fixture.invalid -p 443 -ri 12001;",
        b"UserEntry:arc -h 42.7 -p 98;tcp -h fixture.invalid -p 443;"] {
        let (rpc, who, source, reply) = auxiliary(raw);
        let parent = RpcConnectionScope::fresh();
        assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
            None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
        assert_eq!(rpc.phase(), ArcPhase::Initial);
        assert!(!rpc.is_pending(reply.call_id()));
        assert!(source.is_open() && parent.is_open());
    }
}

#[test]
fn client_target_does_not_invent_empty_direct_or_repair_unsupported_syntax() {
    let router = policy();
    // No current parser path produces Direct(empty); testing that distinction
    // must not create a synthetic unsealed bootstrap or modify the parser.
    for raw in [b"UserEntry:".as_slice(), b"UserEntry:;", b"UserEntry@", b"UserEntry@adapter$",
        b"UserEntry@adapter$locator$other", b"UserEntry@adapter:tcp -h fixture.invalid -p 443;"] {
        assert!(parse_object_reference(raw).is_err());
        let (_rpc, who, source, reply) = auxiliary(raw);
        let parent = RpcConnectionScope::fresh();
        assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
            None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    }
}

#[test]
fn client_target_absent_auxiliary_stays_absent_despite_account_and_metadata_fields() {
    let (rpc, who, source, reply) = auxiliary(b"");
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    assert!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").unwrap().is_none());
    assert!(rpc.user_entry_auth().is_err());
    assert_eq!(rpc.phase(), ArcPhase::Initial);
}

#[test]
fn client_target_capture_refuses_foreign_closed_or_revoked_original_source() {
    let router = policy();
    let (mut rpc, who, source, reply) = auxiliary(b"UserEntry");
    let parent = RpcConnectionScope::fresh();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &owner(), &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &RpcConnectionScope::fresh(),
        &parent, &router, None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    rpc.close();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    let (rpc, who, source, reply) = auxiliary(b"UserEntry@adapter$locator");
    source.mark_session_revoked();
    assert!(source.is_open());
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    assert_eq!(rpc.phase(), ArcPhase::Initial);
}

#[test]
fn client_target_capture_requires_distinct_open_nonrevoked_parent() {
    let (rpc, who, source, reply) = auxiliary(b"UserEntry");
    let router = policy();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &source.clone(), &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    let closed = RpcConnectionScope::fresh();
    closed.close();
    let revoked = RpcConnectionScope::fresh();
    revoked.mark_session_revoked();
    assert!(revoked.is_open());
    for parent in [&closed, &revoked] {
        assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, parent, &router,
            None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    }
    assert_eq!(rpc.phase(), ArcPhase::Initial);
    assert!(source.is_open());
}

#[test]
fn client_target_owns_exact_parsed_raw_move_after_source_and_inputs_drop() {
    let mut input = b" \t prefix/UserEntry@adapter$locator \r\n".to_vec();
    let mut realm = b"opaque realm: not a TLS authority".to_vec();
    let mut cc = b"fixture-cc\0\xff".to_vec();
    let (mut rpc, who, source, reply) = auxiliary(&input);
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    let target = NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, &realm, &cc).unwrap().unwrap();
    let retained = target.retained_bytes();
    let raw_pointer = target.reference.original_factory_input().as_ptr();
    assert_eq!(retained, target.reference.retained_bytes() + realm.len() + cc.len());
    assert!(retained > input.len() + realm.len() + cc.len());
    input.fill(b'x'); realm.fill(b'x'); cc.fill(b'x');
    drop(input); drop(realm); drop(cc); drop(reply);
    rpc.close(); drop(rpc);
    assert!(!source.is_open());
    assert_eq!(target.factory_input_for(&who, &parent, &router).unwrap(),
        b" \t prefix/UserEntry@adapter$locator \r\n");
    assert_eq!(target.realm_for(&who, &parent, &router).unwrap(), b"opaque realm: not a TLS authority");
    assert_eq!(target.cc_for(&who, &parent, &router).unwrap(), b"fixture-cc\0\xff");
    for _ in 0..3 {
        assert_eq!(target.factory_input_for(&who, &parent, &router).unwrap().as_ptr(), raw_pointer);
        assert_eq!(target.retained_bytes(), retained);
    }
    parent.close();
    assert_denied(&target, &who, &parent, &router);
    // Accounting describes still-owned private DATA even when access is denied.
    assert_eq!(target.retained_bytes(), retained);
}

#[test]
fn client_target_all_accessors_refuse_owner_parent_router_replacement_and_parent_close() {
    let (mut rpc, who, source, reply) = auxiliary(b" UserEntry@adapter$locator ");
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    let target = capture(&reply, &who, &source, &parent, &router);
    source.mark_session_revoked();
    assert!(source.is_open());
    assert!(target.matches(&who, &parent, &router));
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    rpc.close();
    assert!(target.matches(&who, &parent.clone(), &router));
    assert_denied(&target, &owner(), &parent, &router);
    assert_denied(&target, &who, &RpcConnectionScope::fresh(), &router);
    assert_denied(&target, &who, &parent, &policy());
    assert_denied(&target, &who, &source, &router);
    parent.clone().close();
    assert_denied(&target, &who, &parent, &router);
    assert_denied(&target, &who, &RpcConnectionScope::fresh(), &policy());
}

#[test]
fn client_target_parent_revocation_denies_data_and_current_auth_without_closing_scope() {
    let who = owner();
    let (rpc, parent) = current_native_for(&who);
    let auth = rpc.user_entry_auth().unwrap();
    let (mut source_rpc, source, reply) = auxiliary_for(&who, b"UserEntry");
    let router = policy();
    let target = capture(&reply, &who, &source, &parent, &router);
    source_rpc.close();
    assert!(target.matches(&who, &parent, &router) && auth.matches(&who, &parent));
    parent.mark_session_revoked();
    assert!(parent.is_open());
    assert_denied(&target, &who, &parent, &router);
    assert!(!auth.matches(&who, &parent));
    assert!(rpc.user_entry_auth().is_err());
    assert_denied(&target, &who, &RpcConnectionScope::fresh(), &router);
}

#[test]
fn client_target_snapshot_never_authenticates_parent_or_allocates_source_pending_ids() {
    let (mut rpc, who, source, reply) = auxiliary(b"UserEntry");
    let parent_rpc = ArcRpcSession::new(who.clone());
    let parent = parent_rpc.connection_scope();
    let router = policy();
    let target = capture(&reply, &who, &source, &parent, &router);
    assert!(target.matches(&who, &parent, &router));
    assert!(rpc.user_entry_auth().is_err() && parent_rpc.user_entry_auth().is_err());
    assert_eq!(rpc.phase(), ArcPhase::Initial);
    assert_eq!(parent_rpc.phase(), ArcPhase::Initial);
    assert!(!rpc.is_pending(reply.call_id()));
    // Only this explicit test fixture call allocates the next bootstrap id.
    // Snapshot capture and its observations have not allocated any source id.
    let (next, _) = rpc.prepare_bootstrap(b"fixture-appkey", &[]).unwrap();
    assert_eq!(next, reply.call_id() + 1);
    rpc.close();
    assert!(target.matches(&who, &parent, &router));
    assert!(rpc.user_entry_auth().is_err() && parent_rpc.user_entry_auth().is_err());
}

#[test]
fn client_target_current_sdk_auth_is_independent_and_cannot_be_substituted() {
    let who = owner();
    let (mut parent_rpc, parent) = current_native_for(&who);
    let auth = parent_rpc.user_entry_auth().unwrap();
    let (replacement_rpc, replacement_parent) = current_native_for(&who);
    let replacement_auth = replacement_rpc.user_entry_auth().unwrap();
    let (mut source_rpc, source, reply) = auxiliary_for(&who, b" prefix/UserEntry@adapter$locator ");
    let router = policy();
    let target = capture(&reply, &who, &source, &parent, &router);
    source_rpc.close();
    assert!(target.matches(&who, &parent, &router));
    assert!(auth.matches(&who, &parent));
    assert!(!replacement_auth.matches(&who, &parent));
    assert!(!auth.matches(&who, &replacement_parent));
    assert_denied(&target, &who, &replacement_parent, &router);
    assert_denied(&target, &owner(), &parent, &router);
    assert_eq!(parent_rpc.phase(), ArcPhase::Authenticated);
    parent_rpc.close();
    assert_denied(&target, &who, &parent, &router);
    assert!(!auth.matches(&who, &parent));
    assert!(replacement_auth.matches(&who, &replacement_parent));
    assert_denied(&target, &who, &replacement_parent, &router);
}

#[test]
fn client_target_realm_cc_and_combined_input_keep_existing_field_bounds() {
    let (_rpc, who, source, reply) = auxiliary(b"UserEntry");
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"", b"").err(), Some(CodecError::InvalidField));
    let oversized = vec![b'x'; MAX_FIELD_BYTES + 1];
    for (realm, cc) in [(oversized.as_slice(), b"".as_slice()), (b"r".as_slice(), oversized.as_slice())] {
        assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
            None, realm, cc).err(), Some(CodecError::TooLarge));
    }
    let room = MAX_FIELD_BYTES - b"UserEntry".len() - 1;
    let cc = vec![b'x'; room];
    let target = NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"r", &cc).unwrap().unwrap();
    assert_eq!(target.cc_for(&who, &parent, &router).unwrap().len(), room);
    assert_eq!(target.retained_bytes(), target.reference.retained_bytes() + 1 + room);
    let too_much_cc = vec![b'x'; room + 1];
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"r", &too_much_cc).err(), Some(CodecError::TooLarge));
    // Full preparation/account budget admission belongs to the caller; this
    // test makes no claim to have reserved that budget or run an operation.
}

#[test]
fn client_target_reference_decode_key_preserves_exact_plain_factory_string() {
    let raw = b" \t prefix/UserEntry@adapter$locator \r\n";
    let encrypted = fixture_envelope(raw, b"fixture-reference-key", [8; 8]);
    let (_rpc, who, source, reply) = auxiliary(&encrypted);
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    let reference_key = ConfigDecodeKey::supplied(b"fixture-reference-key").unwrap();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    let wrong_key = ConfigDecodeKey::supplied(b"fixture-client-property-key").unwrap();
    assert_eq!(NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        Some(&wrong_key), b"fixture-realm.invalid", b"").err(), Some(CodecError::InvalidField));
    let target = NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        Some(&reference_key), b"fixture-realm.invalid", b"").unwrap().unwrap();
    assert_eq!(target.factory_input_for(&who, &parent, &router), Some(raw.as_slice()));
    assert_ne!(target.factory_input_for(&who, &parent, &router).unwrap(), encrypted.as_slice());
    assert_ne!(target.factory_input_for(&who, &parent, &router).unwrap(),
        target.reference.encode_native().unwrap().expose_to_backend());
}

#[test]
fn client_target_and_original_holders_debug_redact_all_factory_and_metadata_secrets() {
    let (_rpc, who, source, reply) = auxiliary(b" private-prefix/UserEntry@private-adapter$private-locator ");
    let parent = RpcConnectionScope::fresh();
    let router = policy();
    let reference = UserEntryReference::from_bootstrap(&reply, &who, &source, None).unwrap().unwrap();
    let target = NewClientTarget::from_bootstrap(&reply, &who, &source, &parent, &router,
        None, b"private-realm.invalid", b"private-cc").unwrap().unwrap();
    assert_eq!(format!("{target:?}"), "NewClientTarget(<redacted>)");
    let debug = format!("{target:?}{reference:?}{reply:?}{router:?}{:?}{:?}",
        target.reference, target.reference.target());
    for secret in ["private-prefix", "private-prefix/UserEntry@", "private-adapter", "private-locator",
        "private-realm", "private-cc", "fixture-client-target-owner", "12001", "127.0.0.1",
        "wrong-bootstrap-metadata-id", "wrong-property-reference", "AccountEntry"] {
        assert!(!debug.contains(secret));
    }
    parent.close();
    assert_eq!(format!("{target:?}"), "NewClientTarget(<redacted>)");
}

#[test]
fn client_target_original_transfer_moves_exact_storage_only_under_valid_source() {
    let raw = b" \t prefix/UserEntry@adapter$locator \r\n";
    let (mut rpc, who, source, reply) = auxiliary(raw);
    let original = UserEntryReference::from_bootstrap(&reply, &who, &source, None).unwrap().unwrap();
    let pointer = original.reference_for(&who, &source).unwrap().original_factory_input().as_ptr();
    let retained = original.reference_for(&who, &source).unwrap().retained_bytes();
    let transferred = original.into_reference(&who, &source).unwrap();
    assert_eq!(transferred.original_factory_input().as_ptr(), pointer);
    assert_eq!(transferred.retained_bytes(), retained);
    rpc.close();
    assert_eq!(transferred.original_factory_input(), raw);
    // Even this exact moved ObjectReference has no constructor path into a new
    // Client target: only a newly checked matched bootstrap can capture one.
    for refusal in 0..4 {
        let (mut rpc, who, source, reply) = auxiliary(raw);
        let original = UserEntryReference::from_bootstrap(&reply, &who, &source, None).unwrap().unwrap();
        match refusal {
            0 => assert_eq!(original.into_reference(&owner(), &source).err(), Some(CodecError::InvalidField)),
            1 => assert_eq!(original.into_reference(&who, &RpcConnectionScope::fresh()).err(), Some(CodecError::InvalidField)),
            2 => {
                rpc.close();
                assert_eq!(original.into_reference(&who, &source).err(), Some(CodecError::InvalidField));
            },
            _ => {
                source.mark_session_revoked();
                assert!(source.is_open());
                assert_eq!(original.into_reference(&who, &source).err(), Some(CodecError::InvalidField));
            },
        }
    }
}
