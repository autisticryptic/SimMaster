//! Actions-only 原生 RPC owner/scope/correlation；不把离线 codec 当网络授信。
use super::*;
use super::super::{rpc_envelope::{decode_request, encode_reply}, rpc_stream::{Reader, Writer}, session::HostingSession, tests::imported};
fn session() -> (AttemptOwner, ArcRpcSession) {
    let mut owner = HostingSession::new("fixture-offline-line".into()); owner.import(imported(), 100).unwrap();
    let owner = owner.begin_login(100).unwrap(); (owner.clone(), ArcRpcSession::new(owner))
}
fn authenticate(owner: &AttemptOwner, session: &mut ArcRpcSession) {
    let inputs = ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-original-selector",
        phone_login_token: b"fixture-phone-pwd", application_key: b"fixture-app-key", domain_id: b"fixture-domain", application_id: b"fixture-app", device_info_cc: b"", client_properties: &[] };
    let (id, _) = session.prepare_login(inputs, &[]).unwrap();
    let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.bytes(b"fixture-native-user").unwrap(); writer.bytes(b"fixture-arc-auth").unwrap(); writer.properties(&[]).unwrap(); writer.bytes(b"").unwrap();
    let reply = encode_reply(id, 0, writer.finish().expose_to_transport()).unwrap();
    let scope = session.connection_scope();
    assert!(matches!(session.on_reply(owner, &scope, reply.expose_to_transport()).unwrap(), Some(ArcReply::Login(_))));
}
fn empty_page() -> Vec<u8> { let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.u32(0).unwrap(); writer.i64(0).unwrap(); writer.bytes(b"").unwrap(); writer.finish().expose_to_transport().to_vec() }
#[test]
fn offline_request_requires_authenticated_original_material_and_serializes_exact_uid_selector_token() {
    let (owner, mut session) = session(); assert!(session.prepare_offline(-1, &[]).is_err()); authenticate(&owner, &mut session);
    let (id, raw) = session.prepare_offline(-1, &[]).unwrap(); let request = decode_request(raw.expose_to_transport()).unwrap();
    assert_eq!(request.method, b"getOfflineMessages.AccountEntry.Account");
    assert_eq!(request.metadata.iter().find(|property| property.key == b"token").unwrap().value, b"fixture-arc-auth");
    assert!(!request.metadata.iter().any(|property| property.key == b"pwd"));
    let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), b"fixture-native-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-original-selector");
    assert_eq!(reader.i64().unwrap(), -1); assert_eq!(reader.u32().unwrap(), 100); reader.finish().unwrap();
    assert!(session.prepare_offline(0, &[]).is_err()); assert!(session.is_pending(id));
}
#[test]
fn foreign_and_missing_correlations_cannot_create_an_owned_page_or_consume_the_original_pending_request() {
    let (owner, mut session) = session(); authenticate(&owner, &mut session); let scope = session.connection_scope();
    let (id, _) = session.prepare_offline(-1, &[]).unwrap(); let reply = encode_reply(id, 0, &empty_page()).unwrap();
    assert!(session.on_reply(&owner, &RpcConnectionScope::fresh(), &[0xff]).unwrap().is_none());
    let (foreign, _) = self::session(); assert!(session.on_reply(&foreign, &scope, &[0xff]).unwrap().is_none());
    assert!(session.is_pending(id)); let missing = encode_reply(id + 50, 0, &empty_page()).unwrap();
    assert!(session.on_reply(&owner, &scope, missing.expose_to_transport()).unwrap().is_none()); assert!(session.is_pending(id));
    let Some(ArcReply::Offline(page)) = session.on_reply(&owner, &scope, reply.expose_to_transport()).unwrap() else { panic!("expected matched page"); };
    assert!(page.matches(&owner, &scope)); assert!(!page.matches(&foreign, &scope)); assert_eq!(page.call_id(), id);
    assert!(!session.is_pending(id)); assert!(session.on_reply(&owner, &scope, reply.expose_to_transport()).unwrap().is_none());
    scope.close(); assert!(!page.matches(&owner, &scope)); assert!(session.prepare_offline(1, &[]).is_err());
}
#[test]
fn malformed_page_retires_original_scope_and_logout_never_replays_an_offline_request() {
    let (owner, mut session) = session(); authenticate(&owner, &mut session); let scope = session.connection_scope();
    let (id, _) = session.prepare_offline(-1, &[]).unwrap(); let reply = encode_reply(id, 0, &[0xff]).unwrap();
    assert!(session.on_reply(&owner, &scope, reply.expose_to_transport()).is_err()); assert!(!scope.is_open());
    let (owner, mut session) = self::session(); authenticate(&owner, &mut session); let scope = session.connection_scope();
    let (old, _) = session.prepare_offline(-1, &[]).unwrap(); session.prepare_logout(&[]).unwrap();
    let late = encode_reply(old, 0, &empty_page()).unwrap(); assert!(session.on_reply(&owner, &scope, late.expose_to_transport()).unwrap().is_none());
    assert!(session.prepare_offline(1, &[]).is_err());
}
#[test]
fn caller_cannot_replace_native_auth_token_in_offline_metadata() {
    let (owner, mut session) = session(); authenticate(&owner, &mut session);
    for key in [b"pwd".as_slice(), b"token".as_slice()] { assert!(session.prepare_offline(-1, &[RpcProperty { key, value: b"fixture-forged" }]).is_err()); }
    assert!(session.prepare_offline(-1, &[]).is_ok());
}
