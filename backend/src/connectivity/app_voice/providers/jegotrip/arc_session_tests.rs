use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    rpc_batch::read_payload, rpc_envelope::decode_request, rpc_stream::Reader,
    session::{HostingSession, Phase}, tests::imported,
};

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    session.begin_login(100).unwrap()
}

fn inputs<'a>(properties: &'a [RpcProperty<'a>]) -> ArcLoginInputs<'a> {
    ArcLoginInputs { typed_identity: b"username:fixture-user", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selected-session", phone_login_token: b"fixture-phone-token",
        application_key: b"fixture-app-key", domain_id: b"fixture-domain-id", application_id: b"fixture-app-id",
        device_info_cc: b"fixture-cc", client_properties: properties }
}

fn reply(id: i32, result: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&(result.len() as u32).to_be_bytes());
    bytes.extend_from_slice(result);
    bytes
}

fn login_result(success: bool) -> Vec<u8> {
    let mut bytes = vec![0, u8::from(success)];
    for value in [b"fixture-returned-user".as_slice(), b"fixture-returned-auth-token".as_slice()] {
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(value);
    }
    bytes.extend_from_slice(&[0; 8]);
    bytes
}

fn authenticated() -> (ArcRpcSession, AttemptOwner, RpcConnectionScope) {
    let owner = owner(); let mut rpc = ArcRpcSession::new(owner.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(inputs(&[]), &[]).unwrap();
    assert!(matches!(rpc.on_reply(&owner, &scope, &reply(id, &login_result(true))).unwrap(), Some(ArcReply::Login(_))));
    (rpc, owner, scope)
}

#[test]
fn original_logout_uses_returned_user_selected_id_and_auth_token_without_credential_rebinding() {
    let (mut rpc,owner,scope)=authenticated();let (id,bytes)=rpc.prepare_logout(&[RpcProperty{key:b"id",value:b"#original-account"}]).unwrap();
    let request=decode_request(bytes.expose_to_transport()).unwrap();assert_eq!(request.method,LOGOUT_SESSION_METHOD.as_bytes());
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-returned-auth-token");assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));
    let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.bytes().unwrap(),b"fixture-returned-user");
    assert_eq!(reader.bytes().unwrap(),b"fixture-selected-session");reader.finish().unwrap();assert_eq!(rpc.phase(),ArcPhase::LogoutPending);
    let receipt=match rpc.on_reply(&owner,&scope,&reply(id,b"\0\x01\0\0\0\0")).unwrap().unwrap() { ArcReply::Logout(receipt)=>receipt,_=>panic!("logout result") };
    assert!(receipt.matches(&owner,&scope));assert!(receipt.accepted());assert_eq!(receipt.call_id(),id);assert_eq!(rpc.phase(),ArcPhase::LoggedOut);
    assert!(rpc.authentication.is_none());assert!(rpc.prepare_logout(&[]).is_err());assert!(!format!("{receipt:?}").contains("fixture"));
}

#[test]
fn invalid_logout_does_not_retire_original_pending_swap_but_valid_logout_does() {
    let (mut rpc,owner,scope)=authenticated();let payload=BatchPayload{fields:[0;6],requests:BTreeMap::new(),results:BTreeMap::new()};
    let (swap,_) = rpc.prepare_swap_calls(&payload,&[]).unwrap();assert!(rpc.prepare_logout(&[RpcProperty{key:b"token",value:b"foreign"}]).is_err());
    assert!(rpc.is_pending(swap));assert_eq!(rpc.phase(),ArcPhase::Authenticated);
    let (logout,_) = rpc.prepare_logout(&[]).unwrap();assert!(!rpc.is_pending(swap));assert!(rpc.is_pending(logout));
    assert!(rpc.on_reply(&owner,&scope,&reply(swap,b"bad old swap")).unwrap().is_none());assert_eq!(rpc.phase(),ArcPhase::LogoutPending);
}

#[test]
fn logout_requires_authenticated_original_scope_and_unknown_stale_replies_cannot_complete_it() {
    let mut initial=ArcRpcSession::new(owner());assert!(initial.prepare_logout(&[]).is_err());
    let (mut rpc,original,scope)=authenticated();let (id,_) = rpc.prepare_logout(&[]).unwrap();
    assert!(rpc.on_reply(&owner(),&scope,b"bad").unwrap().is_none());
    assert!(rpc.on_reply(&original,&RpcConnectionScope::fresh(),b"bad").unwrap().is_none());
    assert!(rpc.on_reply(&original,&scope,&reply(id+1,b"\0\x01\0\0\0\0")).unwrap().is_none());assert_eq!(rpc.phase(),ArcPhase::LogoutPending);
    rpc.retire_request(id);assert_eq!(rpc.phase(),ArcPhase::Closed);assert!(!scope.is_open());
}

#[test]
fn false_logout_result_is_not_a_success_receipt_and_malformed_logout_is_terminal() {
    let (mut rpc,owner,scope)=authenticated();let (id,_) = rpc.prepare_logout(&[]).unwrap();
    let receipt=match rpc.on_reply(&owner,&scope,&reply(id,b"\0\0\0\0\0\0")).unwrap().unwrap() { ArcReply::Logout(receipt)=>receipt,_=>panic!("logout") };
    assert!(!receipt.accepted());assert_eq!(rpc.phase(),ArcPhase::LoggedOut);assert!(rpc.authentication.is_none());
    let (mut rpc,owner,scope)=authenticated();let (id,_) = rpc.prepare_logout(&[]).unwrap();
    assert!(matches!(rpc.on_reply(&owner,&scope,&reply(id,b"\0\x01\0")).unwrap(),Some(ArcReply::Failure(_))));assert_eq!(rpc.phase(),ArcPhase::Closed);
}

#[test]
fn login_keeps_password_selected_session_and_returned_token_in_distinct_layers() {
    let owner = owner(); let mut rpc = ArcRpcSession::new(owner.clone()); let scope = rpc.connection_scope();
    let routing = [RpcProperty { key: b"id", value: b"#fixture" }, RpcProperty { key: b"__magic", value: &[0, 0xff] }];
    let (id, bytes) = rpc.prepare_login(inputs(&[]), &routing).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    assert_eq!(request.method, LOGIN_SESSION_METHOD.as_bytes());
    assert_eq!(request.metadata.iter().find(|field| field.key == b"pwd").unwrap().value, b"fixture-phone-token");
    assert!(request.metadata.iter().all(|field| field.key != b"token"));
    let mut reader = Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(), b"[username:fixture-user@fixture.invalid]");
    assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
    let properties = reader.properties().unwrap(); reader.finish().unwrap();
    assert_eq!(properties.iter().find(|field| field.key == b"ForceFlag").unwrap().value, b"0");
    assert_eq!(properties.iter().find(|field| field.key == b"__appKey").unwrap().value, b"fixture-app-key");
    let callback = match rpc.on_reply(&owner, &scope, &reply(id, &login_result(true))).unwrap().unwrap() {
        ArcReply::Login(callback) => callback, _ => panic!("login callback expected"),
    };
    assert_eq!(callback.owner, owner);
    assert_eq!(callback.callback, Callback::SdkLogin { success: true });
    let payload = BatchPayload { fields: [1, 2, 3, 4, 5, 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    let (_, bytes) = rpc.prepare_swap_calls(&payload, &routing).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    assert_eq!(request.metadata.iter().find(|field| field.key == b"token").unwrap().value, b"fixture-returned-auth-token");
    assert!(request.metadata.iter().all(|field| field.key != b"pwd"));
    let mut reader = Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(), b"fixture-returned-user");
    assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
    assert_eq!(read_payload(&mut reader).unwrap().fields, payload.fields);
    reader.finish().unwrap();
}

#[test]
fn explicit_native_login_properties_override_only_the_verified_keys_and_force_stays_off() {
    let properties = [RpcProperty { key: b"DeviceId", value: b"fixture-device" },
        RpcProperty { key: b"__appKey", value: b"old" }, RpcProperty { key: b"custom", value: &[0, 0xff] }];
    let mut rpc = ArcRpcSession::new(owner());
    let (_, bytes) = rpc.prepare_login(inputs(&properties), &[]).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    let mut reader = Reader::versioned(request.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
    let decoded = reader.properties().unwrap();
    assert_eq!(decoded.iter().filter(|field| field.key == b"__appKey").count(), 1);
    assert_eq!(decoded.iter().find(|field| field.key == b"__appKey").unwrap().value, b"fixture-app-key");
    assert_eq!(decoded.iter().find(|field| field.key == b"custom").unwrap().value, &[0, 0xff]);
    assert_eq!(decoded.iter().find(|field| field.key == b"DeviceId").unwrap().value, b"fixture-device");
    let forced = [RpcProperty { key: b"ForceFlag", value: b"1" }];
    assert_eq!(ArcRpcSession::new(owner()).prepare_login(inputs(&forced), &[]).err(), Some(CodecError::InvalidField));
}

#[test]
fn caller_cannot_override_phone_or_auth_token_and_incomplete_inputs_are_not_filled() {
    for key in [b"pwd".as_slice(), b"token".as_slice()] {
        let routing = [RpcProperty { key, value: b"injected" }];
        let mut rpc = ArcRpcSession::new(owner());
        assert_eq!(rpc.prepare_login(inputs(&[]), &routing).err(), Some(CodecError::InvalidField));
        assert_eq!(rpc.phase(), ArcPhase::Initial);
    }
    let mut missing = inputs(&[]); missing.selected_session_id = b"";
    assert_eq!(ArcRpcSession::new(owner()).prepare_login(missing, &[]).err(), Some(CodecError::InvalidField));
    let mut encrypted = inputs(&[]); encrypted.domain_id = b"ENC(unresolved)";
    assert_eq!(ArcRpcSession::new(owner()).prepare_login(encrypted, &[]).err(), Some(CodecError::InvalidField));
    let mut encrypted = inputs(&[]); encrypted.typed_identity = b"ENC(unresolved)";
    assert_eq!(ArcRpcSession::new(owner()).prepare_login(encrypted, &[]).err(), Some(CodecError::InvalidField));
    let mut missing_domain = inputs(&[]); missing_domain.typed_identity = b"[fixture]"; missing_domain.domain_name = b"";
    assert_eq!(ArcRpcSession::new(owner()).prepare_login(missing_domain, &[]).err(), Some(CodecError::InvalidField));
}

#[test]
fn stale_owner_stale_connection_unknown_and_duplicate_login_replies_never_rebind() {
    let original = owner(); let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(inputs(&[]), &[]).unwrap();
    let bytes = reply(id, &login_result(true));
    assert!(rpc.on_reply(&owner(), &scope, &[0xff]).unwrap().is_none());
    let old_scope = ArcRpcSession::new(original.clone()).connection_scope();
    assert!(rpc.on_reply(&original, &old_scope, &[0xff]).unwrap().is_none());
    assert!(rpc.on_reply(&original, &scope, &reply(id + 1, &login_result(true))).unwrap().is_none());
    assert_eq!(rpc.phase(), ArcPhase::LoginPending);
    assert!(matches!(rpc.on_reply(&original, &scope, &bytes).unwrap(), Some(ArcReply::Login(_))));
    assert!(rpc.on_reply(&original, &scope, &bytes).unwrap().is_none());
}

#[test]
fn retired_login_or_malformed_reply_is_terminal_not_an_auth_retry() {
    let original = owner(); let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(inputs(&[]), &[]).unwrap();
    rpc.retire_request(id);
    assert_eq!(rpc.phase(), ArcPhase::Closed);
    assert!(rpc.on_reply(&original, &scope, &reply(id, &login_result(true))).unwrap().is_none());
    assert!(rpc.prepare_login(inputs(&[]), &[]).is_err());
    let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    rpc.prepare_login(inputs(&[]), &[]).unwrap();
    assert_eq!(rpc.on_reply(&original, &scope, &[0]).err(), Some(CodecError::Truncated));
    assert_eq!(rpc.phase(), ArcPhase::Closed);
}

#[test]
fn rejected_login_or_terminal_error_cannot_authenticate() {
    for bad in [login_result(false), {
        let mut bytes = login_result(true); bytes.truncate(bytes.len() - 4); bytes.extend_from_slice(&[0, 0, 0, 1, b'e']); bytes
    }] {
        let original = owner(); let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
        let (id, _) = rpc.prepare_login(inputs(&[]), &[]).unwrap();
        let callback = match rpc.on_reply(&original, &scope, &reply(id, &bad)).unwrap().unwrap() {
            ArcReply::Failure(callback) => callback, _ => panic!("failure expected"),
        };
        assert_eq!(callback.owner, original);
        assert_eq!(callback.callback, Callback::Failure(BackendFailure::AuthenticationRejected));
        assert_eq!(rpc.phase(), ArcPhase::Closed);
    }
}

#[test]
fn bootstrap_is_owned_sequential_and_does_not_authenticate_or_mark_hosted() {
    let mut hosting = HostingSession::new("line-a".into());
    hosting.import(imported(), 100).unwrap();
    let original = hosting.begin_login(100).unwrap();
    let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    let (id, bytes) = rpc.prepare_bootstrap(b"fixture-key", &[]).unwrap();
    assert_eq!(decode_request(bytes.expose_to_transport()).unwrap().method, GET_APP_CONFIGS_METHOD.as_bytes());
    assert!(rpc.prepare_login(inputs(&[]), &[]).is_err());
    let mut result = vec![0, 1, 0, 0, 0, 0];
    for value in [b"fixture-entry".as_slice(), b"".as_slice(), b"".as_slice(), b"".as_slice()] {
        result.extend_from_slice(&(value.len() as u32).to_be_bytes()); result.extend_from_slice(value);
    }
    assert!(matches!(rpc.on_reply(&original, &scope, &reply(id, &result)).unwrap(), Some(ArcReply::Bootstrap(_))));
    assert_eq!(rpc.phase(), ArcPhase::Initial);
    assert!(rpc.prepare_login(inputs(&[]), &[]).is_ok());
    assert_eq!(hosting.phase(), Phase::SdkLoginPending);
    assert!(!hosting.incoming_call_ready());
    assert!(!hosting.incoming_message_ready());
}

#[test]
fn swap_requires_authenticated_original_session_and_false_batch_delivers_no_nested_data() {
    let payload = BatchPayload { fields: [0; 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    assert!(ArcRpcSession::new(owner()).prepare_swap_calls(&payload, &[]).is_err());
    let (mut rpc, owner, scope) = authenticated();
    let (id, _) = rpc.prepare_swap_calls(&payload, &[]).unwrap();
    let mut result = vec![0, 0]; result.extend_from_slice(&[0; 24]);
    result.extend_from_slice(&[0; 4]); result.extend_from_slice(&1u32.to_be_bytes());
    result.extend_from_slice(&1i32.to_be_bytes()); result.extend_from_slice(&[0; 4]);
    result.extend_from_slice(&6u32.to_be_bytes()); result.extend_from_slice(&[0, 1, 0, 0, 0, 0]);
    result.extend_from_slice(&[0; 8]);
    let false_envelope = reply(id, &result);
    assert!(matches!(rpc.on_reply(&owner, &scope, &false_envelope).unwrap(), Some(ArcReply::Failure(_))));
    assert_eq!(rpc.phase(), ArcPhase::Closed);
    result[1] = 1;
    assert!(rpc.on_reply(&owner, &scope, &reply(id, &result)).unwrap().is_none());
}

#[test]
fn swap_id_exhaustion_and_local_pending_bounds_never_reuse_old_ids() {
    let payload = BatchPayload { fields: [0; 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    let (mut rpc, _, _) = authenticated();
    for _ in 0..MAX_MAP_ENTRIES { rpc.prepare_swap_calls(&payload, &[]).unwrap(); }
    assert_eq!(rpc.prepare_swap_calls(&payload, &[]).err(), Some(CodecError::TooLarge));
    let (mut rpc, _, _) = authenticated(); rpc.next_id = Some(i32::MAX);
    let (last, _) = rpc.prepare_swap_calls(&payload, &[]).unwrap(); rpc.retire_request(last);
    assert_eq!(last, i32::MAX);
    assert_eq!(rpc.prepare_swap_calls(&payload, &[]).err(), Some(CodecError::InvalidCallId));
}

#[test]
fn arc_debug_redacts_inputs_authentication_reply_and_inflight_metadata() {
    assert!(!format!("{:?}", inputs(&[])).contains("fixture"));
    let (mut rpc, owner, scope) = authenticated();
    assert!(!format!("{rpc:?}").contains("fixture"));
    let payload = BatchPayload { fields: [0; 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    let (id, _) = rpc.prepare_swap_calls(&payload, &[]).unwrap();
    let mut result = vec![0, 1]; result.extend_from_slice(&[0; 24]); result.extend_from_slice(&[0; 4]);
    result.extend_from_slice(&1u32.to_be_bytes()); result.extend_from_slice(&1i32.to_be_bytes());
    result.extend_from_slice(&[0; 4]); result.extend_from_slice(&18u32.to_be_bytes());
    result.extend_from_slice(b"fixture-reply-data"); result.extend_from_slice(&[0; 8]);
    assert!(!format!("{:?}", rpc.on_reply(&owner, &scope, &reply(id, &result)).unwrap()).contains("fixture"));
}

#[test]
fn selected_session_id_is_bounded_before_submission_and_failure_does_not_consume_an_id() {
    let mut rpc = ArcRpcSession::new(owner());
    let too_long = [b's'; 128]; let accepted = [b's'; 127];
    let mut login = inputs(&[]); login.selected_session_id = &too_long;
    assert_eq!(rpc.prepare_login(login, &[]).err(), Some(CodecError::TooLarge));
    assert_eq!(rpc.phase(), ArcPhase::Initial);
    let mut login = inputs(&[]); login.selected_session_id = &accepted;
    assert_eq!(rpc.prepare_login(login, &[]).unwrap().0, 1);
}
