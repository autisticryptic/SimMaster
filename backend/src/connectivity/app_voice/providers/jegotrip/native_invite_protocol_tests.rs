//! Actions-only exact original Invite tickets; these component claims are not
//! actual account consumption, a device test, SDK readiness or media success.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{native_receiver::{NativeReceiverConfig,NativeReceiverPolicy,NativeReceiverSetup},
    native_invite_notifications::{NativeInviteConfig,ConsumerClaim,IncomingInviteNotification,InviteReplySource},
    offline_backend::CallCompletion,arc_protocol::OwnedCallPackets};
use std::time::{Duration,Instant};
#[path="native_invite_claim_tests.rs"]
mod claims;
const ID:u64=0xf123_4567_89ab_cdef;
fn component_claim(invite:&IncomingInviteNotification)->std::sync::Arc<ConsumerClaim>{ConsumerClaim::fixture_consumed(invite.owner(),&invite.connection_scope(),invite.notification_id(),invite.fingerprint(),invite.deadline())}
fn props<'a>(call:&'a[u8],caller:&'a[u8])->Vec<RpcProperty<'a>>{vec![RpcProperty{key:b"Notify.Command",value:b"Invite"},RpcProperty{key:b"Notify.CallId",value:call},RpcProperty{key:b"Notify.Caller",value:caller}]}
fn args(id:u64,properties:&[RpcProperty<'_>])->Vec<u8>{let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();w.properties(properties).unwrap();w.finish().expose_to_transport().to_vec()}
fn context(enabled:bool)->(AttemptOwner,RpcConnectionScope,OfflineBackend){
    let original=owner();let mut backend=OfflineBackend::new(original.clone());backend.claim_common_transport().unwrap();backend.bind_common_identity(b"AccountEntry").unwrap();let scope=backend.connection_scope();
    let policy=NativeReceiverPolicy::capture(&NativeReceiverConfig{im_messages:None,direct_domains:None,invite_notifications:enabled.then_some(NativeInviteConfig{timeout_ms:30000})}).unwrap();
    backend.arm_native_receivers(NativeReceiverSetup::capture_native_control(policy,true).unwrap()).unwrap();
    let login=backend.prepare_login(login(),&[]).unwrap();backend.on_plaintext_packet(&original,&scope,&reply_packet(login.call_id,&login_result()),0).unwrap();backend.configure_native_receivers(7).unwrap();
    (original,scope,backend)
}
fn packet(id:i32,body:&[u8],token:Option<&[u8]>,magic:&[u8])->Vec<u8>{
    let mut metadata=vec![RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"__magic",value:magic}];if let Some(token)=token{metadata.push(RpcProperty{key:b"token",value:token});}
    let request=encode_request(id,&metadata,NOTIFY_CALLS_METHOD.as_bytes(),body).unwrap();let mut p=vec![0];p.extend_from_slice(request.expose_to_transport());p
}
fn take(events:Vec<OfflineEvent>)->IncomingInviteNotification{assert_eq!(events.len(),1);match events.into_iter().next().unwrap(){OfflineEvent::InviteNotification(invite)=>*invite,_=>panic!("original deferred Invite only")}}
fn incoming(backend:&mut OfflineBackend,owner:&AttemptOwner,scope:&RpcConnectionScope,rpc:i32,id:u64)->IncomingInviteNotification{
    take(backend.on_plaintext_packet(owner,scope,&packet(rpc,&args(id,&props(b"opaque-call",b"opaque-caller")),Some(b"fixture-auth"),&rpc.to_be_bytes()),rpc as u64).unwrap())
}
fn direct_result(value:Option<CallCompletion>)->Vec<u8>{let Some(CallCompletion::Direct(packets))=value else{panic!("DIRECT result")};assert_eq!(packets.packets.len(),1);let bytes=packets.packets[0].expose_to_transport();assert_eq!(bytes[0],1);decode_reply(&bytes[1..]).unwrap().result_stream.to_vec()}
fn batch_packet(outer:&OwnedCallPackets,arguments:&[u8])->Vec<u8>{
    let bytes=outer.packets[0].expose_to_transport();let request=decode_request(&bytes[1..]).unwrap();let mut r=Reader::versioned(request.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();r.finish().unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [99,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{w.u32(word).unwrap();}
    w.u32(1).unwrap();w.i32(1).unwrap();w.bytes(NOTIFY_CALLS_METHOD.as_bytes()).unwrap();w.bytes(arguments).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"}]).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();reply_packet(request.call_id,w.finish().expose_to_transport())
}
fn batch_incoming(backend:&mut OfflineBackend,owner:&AttemptOwner,scope:&RpcConnectionScope)->IncomingInviteNotification{
    let outer=backend.prepare_swap(0,&[]).unwrap();let events=backend.on_plaintext_packet(owner,scope,&batch_packet(&outer,&args(ID,&props(b"opaque-call",b"opaque-caller"))),1).unwrap();
    assert!(!events.iter().any(|e|matches!(e,OfflineEvent::Outgoing(_)|OfflineEvent::ClientEx(_)|OfflineEvent::Message(_)|OfflineEvent::Call(_))));
    events.into_iter().find_map(|e|if let OfflineEvent::InviteNotification(invite)=e{Some(*invite)}else{None}).unwrap()
}
#[test]
fn explicit_invite_retains_direct_receipt_without_immediate_ack_call_hint_or_media(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,91,ID);assert!(invite.matches(&owner,&scope));assert_eq!(invite.notification_id(),ID);assert!(matches!(invite.ticket().source(),InviteReplySource::Direct(_)));assert_eq!(backend.phase(),ArcPhase::Authenticated);assert!(!scope.session_revoked());
}
#[test]
fn default_native_invite_still_nacks_without_parsing_business_values(){
    let(owner,scope,mut backend)=context(false);let result=backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(ID,&props(&[255,0,1],b"AccountAlive:stcp -h fixture.invalid -p 1;")),Some(b"fixture-auth"),b"default"),1).unwrap();assert_eq!(control_result(&result),[0;6]);assert_eq!(result.len(),1);assert!(scope.is_open());
}
#[test]
fn explicit_invite_needs_current_arc_token_not_phone_app_missing_or_prefix(){
    for token in [None,Some(b"fixture-phone".as_slice()),Some(b"fixture-auth-extra"),Some(b"fixture-app")]{let(owner,scope,mut backend)=context(true);let result=backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(ID,&props(b"c",b"p")),token,b"token"),1).unwrap();assert_eq!(control_result(&result),[0;6]);assert_eq!(result.len(),1);assert!(scope.is_open());}
}
#[test]
fn raw_invite_ticket_cannot_claim_positive_consumption(){let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);assert!(backend.complete_invite_notification(invite.ticket(),true).is_err());let claim=ConsumerClaim::new(&owner,&scope,ID,invite.fingerprint(),invite.deadline());assert!(backend.complete_invite_notification(&invite.ticket().clipped_to(invite.deadline(),&claim),true).is_err());assert!(scope.is_open());}
#[test]
fn component_consumption_claim_uses_original_direct_bool_footer_not_notifyend_rpc(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,41,ID);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);
    assert_eq!(direct_result(backend.complete_invite_notification(&ticket,true).unwrap()),[0,1,0,0,0,0]);assert!(backend.complete_invite_notification(&ticket,true).unwrap().is_none());assert_eq!(backend.phase(),ArcPhase::Authenticated);
}
#[test]
fn negative_invite_completion_requires_no_claim_and_keeps_original_scope(){let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,11,ID);assert_eq!(direct_result(backend.complete_invite_notification(invite.ticket(),false).unwrap()),[0;6]);assert!(scope.is_open());}
#[test]
fn foreign_owner_scope_and_same_bits_never_transfer_original_reply_ticket(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let(foreign,foreign_scope,mut other)=context(true);let second=incoming(&mut other,&foreign,&foreign_scope,1,ID);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);
    assert!(!ticket.matches(&foreign,&foreign_scope));assert!(!invite.matches(&owner,&RpcConnectionScope::fresh()));assert!(other.complete_invite_notification(&ticket,true).unwrap().is_none());assert_eq!(direct_result(other.complete_invite_notification(second.ticket(),false).unwrap()),[0;6]);
}
#[test]
fn pending_invite_direct_replays_and_binary_magic_do_not_duplicate_delivery(){
    let(owner,scope,mut backend)=context(true);let body=args(ID,&props(b"c",b"p"));let p=packet(1,&body,Some(b"fixture-auth"),b"binary\0magic");take(backend.on_plaintext_packet(&owner,&scope,&p,1).unwrap());assert!(backend.on_plaintext_packet(&owner,&scope,&p,2).unwrap().is_empty());assert!(backend.on_plaintext_packet(&owner,&scope,&packet(2,&body,Some(b"fixture-auth"),b"binary\0magic"),3).unwrap().is_empty());
}
#[test]
fn distinct_direct_rpcs_for_same_notification_keep_separate_original_holders(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,11,ID);let second=incoming(&mut backend,&owner,&scope,12,ID);assert_eq!(first.fingerprint(),second.fingerprint());let claim=component_claim(&first);
    for invite in [&first,&second]{assert_eq!(direct_result(backend.complete_invite_notification(&invite.ticket().clipped_to(first.deadline(),&claim),true).unwrap()),[0,1,0,0,0,0]);}
}
#[test]
fn batch_invite_holds_result_until_original_consumer_completes(){
    let(owner,scope,mut backend)=context(true);let invite=batch_incoming(&mut backend,&owner,&scope);assert!(matches!(invite.ticket().source(),InviteReplySource::Batch(_)));let outer=backend.prepare_swap(2,&[]).unwrap();let request=decode_request(&outer.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(request.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let data=n::rpc_batch::read_payload(&mut r).unwrap();assert!(data.results.is_empty());assert!(scope.is_open());
}
#[test]
fn batch_invite_ticket_cannot_be_completed_as_sms_or_raw_call_result(){
    let(owner,scope,mut backend)=context(true);let invite=batch_incoming(&mut backend,&owner,&scope);let InviteReplySource::Batch(inner)=invite.ticket().source()else{panic!("batch")};assert!(backend.complete_message(inner,true).is_err());assert!(backend.complete_invite_notification(invite.ticket(),true).is_err());
}
#[test]
fn batch_invite_positive_queues_exact_bool_and_keeps_guard_until_peer_ack(){
    let(owner,scope,mut backend)=context(true);let invite=batch_incoming(&mut backend,&owner,&scope);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);assert!(matches!(backend.complete_invite_notification(&ticket,true).unwrap(),Some(CallCompletion::BatchQueued)));
    let outer=backend.prepare_swap(2,&[]).unwrap();let request=decode_request(&outer.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-selector");let payload=n::rpc_batch::read_payload(&mut r).unwrap();r.finish().unwrap();assert_eq!(payload.results[&1].result_stream,[0,1,0,0,0,0]);assert_eq!(backend.invite_write_guard().0.len(),1);assert_eq!(backend.invite_write_guard().1,Some(ticket.completion_deadline()));
}
#[test]
fn cancelled_batch_consumer_does_not_replay_unknown_positive_or_replace_it_with_nack(){
    let(owner,scope,mut backend)=context(true);let invite=batch_incoming(&mut backend,&owner,&scope);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);backend.complete_invite_notification(&ticket,true).unwrap();claim.cancel();assert!(backend.prepare_swap(2,&[]).is_err());assert!(!scope.is_open());
}
#[test]
fn direct_consumer_revocation_cannot_be_undone_by_late_positive_claim(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);claim.cancel();assert!(backend.complete_invite_notification(&ticket,true).is_err());assert_eq!(direct_result(backend.complete_invite_notification(&ticket,false).unwrap()),[0;6]);assert!(scope.is_open());
}
#[test]
fn coalesced_holder_deadline_can_only_clip_and_never_extend_original_window(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let claim=component_claim(&invite);let late=invite.ticket().clipped_to(invite.deadline()+Duration::from_secs(60),&claim);assert_eq!(late.completion_deadline(),invite.deadline());let expired=late.clipped_to(Instant::now(),&claim);assert!(backend.complete_invite_notification(&expired,true).is_err());assert!(backend.complete_invite_notification(&late.clipped_to(expired.completion_deadline(),&claim),true).is_err());
}
#[test]
fn all_truncated_versioned_invites_and_trailing_data_remain_unacknowledged(){
    let body=args(ID,&props(b"c",b"p"));let mut cases=(0..body.len()).map(|len|body[..len].to_vec()).collect::<Vec<_>>();let mut trailing=body.clone();trailing.push(0);cases.push(trailing);let mut version=body;version[4]=1;cases.push(version);
    for body in cases{let(owner,scope,mut backend)=context(true);assert!(backend.on_plaintext_packet(&owner,&scope,&packet(1,&body,Some(b"fixture-auth"),b"bad"),1).is_err());assert!(!scope.session_revoked());}
}
#[test]
fn unknown_invite_alias_extra_missing_and_cross_command_keys_still_fail_closed(){
    let mut cases=Vec::new();for command in [b"invite".as_slice(),b"Invite.extra",b"Invite\0"]{let mut p=props(b"c",b"p");p[0].value=command;cases.push(p);}let mut extra=props(b"c",b"p");extra.push(RpcProperty{key:b"url",value:b"unapproved-route"});cases.push(extra);let mut missing=props(b"c",b"p");missing.pop();cases.push(missing);
    for p in cases{let(owner,scope,mut backend)=context(true);assert!(backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(ID,&p),Some(b"fixture-auth"),b"bad"),1).is_err());assert!(!scope.is_open());}
}
#[test]
fn enabled_invite_bounds_each_opaque_value_before_owned_allocation(){
    let large=vec![0xff;n::native_invite_notifications::MAX_INVITE_VALUE_BYTES+1];for p in [props(&large,b"p"),props(b"c",&large)]{let(owner,scope,mut backend)=context(true);assert!(backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(ID,&p),Some(b"fixture-auth"),b"large"),1).is_err());assert!(!scope.session_revoked());}
}
#[test]
fn full_notification_id_is_data_not_outer_i32_and_zero_is_not_completable(){
    for id in [1,0x1_0000_0001,u64::MAX]{let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,id);assert_eq!(invite.notification_id(),id);assert!(invite.identifies(&owner,&scope,n::rpc_calls::NativeCallId::from_bits(id)));assert!(!invite.identifies(&owner,&scope,n::rpc_calls::NativeCallId::from_bits(id.wrapping_sub(1))));}
    let(owner,scope,mut backend)=context(true);assert!(backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(0,&props(b"c",b"p")),Some(b"fixture-auth"),b"zero"),1).is_err());
}
#[test]
fn message_im_box_is_not_swallowed_by_invite_installation_or_routed_as_sms(){
    let(owner,scope,mut backend)=context(true);let fields=[RpcProperty{key:b"Notify.Command",value:b"Message"},RpcProperty{key:b"Notify.MsgIdx",value:&[255,0,1]},RpcProperty{key:b"Notify.Box",value:b"Org/opaque"}];let events=backend.on_plaintext_packet(&owner,&scope,&packet(1,&args(ID,&fields),Some(b"fixture-auth"),b"im"),1).unwrap();assert_eq!(control_result(&events),[0;6]);assert_eq!(events.len(),1);assert!(scope.is_open());
}
#[test]
fn closed_and_session_revoked_scopes_drop_invite_completion_without_rebinding(){
    for revoke in [false,true]{let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let claim=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&claim);if revoke{scope.mark_session_revoked();}else{scope.close();}assert!(!invite.matches(&owner,&scope));assert!(backend.complete_invite_notification(&ticket,true).unwrap().is_none());}
}
#[test]
fn invite_policy_requires_explicit_bounded_timeout_and_rejects_unknown_fields(){
    for timeout_ms in [0,99,30001,u64::MAX]{assert!(NativeInviteConfig{timeout_ms}.timeout().is_err());}for timeout_ms in [100,30000]{assert!(NativeInviteConfig{timeout_ms}.timeout().is_ok());}
    for value in [r#"{}"#,r#"{"timeout_ms":1000,"url":"fixture"}"#,r#"true"#]{assert!(serde_json::from_str::<NativeInviteConfig>(value).is_err());}
    let config:NativeReceiverConfig=serde_json::from_str(r#"{}"#).unwrap();assert!(config.invite_notifications.is_none());
}
#[test]
fn invite_setup_cannot_be_armed_on_legacy_control_or_disabled_clientex(){
    let policy=NativeReceiverPolicy::capture(&NativeReceiverConfig{im_messages:None,direct_domains:None,invite_notifications:Some(NativeInviteConfig{timeout_ms:1000})}).unwrap();assert!(NativeReceiverSetup::capture(policy.clone(),true,None).is_err());assert!(NativeReceiverSetup::capture_native_control(policy.clone(),false).is_err());assert!(NativeReceiverSetup::capture_native_control(policy,true).is_ok());
}
#[test]
fn invite_debug_and_fingerprint_do_not_reflect_private_business_values(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let debug=format!("{invite:?} {:?}",invite.ticket());for value in ["opaque","fixture","f123","caller"]{assert!(!debug.contains(value));}assert_eq!(invite.fingerprint().len(),32);assert!(invite.retained_bytes()<=2*n::native_invite_notifications::MAX_INVITE_VALUE_BYTES+128);
}
