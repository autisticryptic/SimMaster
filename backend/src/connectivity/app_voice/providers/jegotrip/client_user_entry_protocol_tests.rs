//! Actions-only Client READ protocol/retention regressions, no original SDK execution.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{user_entry_action::UserEntryAction,user_entry_client::ClientUserEntryQuery,user_entry_client_target::NewClientTarget,
    user_entry_rpc::UserEntryError,im_inbox::ImReceiptGate,arc_session::{ArcLoginInputs,ArcReply,ArcRpcSession,OwnedBootstrapReply},
    rpc_envelope::{encode_reply,decode_request},rpc_stream::{Writer,Reader},router_connector::{RouterPolicy,RouterAnchor},session::{HostingSession,STATUS_REGISTER,STATUS_UNREGISTER}};
use std::sync::{Arc,OnceLock};
fn who()->AttemptOwner{let mut s=HostingSession::new("fixture-client-read".into());s.import(n::tests::imported(),100).unwrap();s.begin_login(100).unwrap()}
fn router()->RouterPolicy{static ROOT:OnceLock<String>=OnceLock::new();let root=ROOT.get_or_init(||n::tls_engine::tests::Certificates::generate("IP:127.0.0.1").root.clone());RouterPolicy::supplied("Router:tls -h 127.0.0.1 -p 12345 -ri 12001;",&[RouterAnchor{router_id:12001,host:"127.0.0.1",port:12345,authority:"127.0.0.1",root,enable_sni:false}]).unwrap()}
fn packet(id:i32,code:u32,result:&[u8])->Vec<u8>{let rpc=encode_reply(id,code,result).unwrap();let mut p=vec![1];p.extend_from_slice(rpc.expose_to_transport());p}
fn body(reference:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&[]).unwrap();w.bytes(b"DestEntry:sarc -h 42.7 -p 98;").unwrap();w.bytes(b"").unwrap();w.bytes(reference).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn source(owner:&AttemptOwner,reference:&[u8])->(ArcRpcSession,RpcConnectionScope,OwnedBootstrapReply){let mut rpc=ArcRpcSession::new(owner.clone());let scope=rpc.connection_scope();let(id,_)=rpc.prepare_bootstrap(b"fixture",&[]).unwrap();let bytes=packet(id,0,&body(reference));let reply=match rpc.on_reply(owner,&scope,&bytes[1..]).unwrap().unwrap(){ArcReply::Bootstrap(value)=>value,_=>panic!("bootstrap")};(rpc,scope,reply)}
fn current()->(OfflineBackend,AttemptOwner,RpcConnectionScope,RouterPolicy){let owner=who();let mut backend=OfflineBackend::new(owner.clone());backend.claim_common_transport().unwrap();backend.bind_common_identity(b"DestEntry").unwrap();let parent=backend.connection_scope();let inputs=ArcLoginInputs{typed_identity:b"username:fixture-user",domain_name:b"fixture.invalid",selected_session_id:b"fixture-selected",phone_login_token:b"fixture-phone",application_key:b"fixture-key",domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"fixture-cc",client_properties:&[]};let call=backend.prepare_login(inputs,&[]).unwrap();let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"7_9").unwrap();w.bytes(b"fixture-auth").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();backend.on_plaintext_packet(&owner,&parent,&packet(call.call_id,0,w.finish().expose_to_transport()),0).unwrap();backend.configure(77,None).unwrap();(backend,owner,parent,router())}
fn query(owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy,raw:&[u8])->Arc<ClientUserEntryQuery>{let(_source,scope,reply)=source(owner,raw);let target=Arc::from(NewClientTarget::from_bootstrap(&reply,owner,&scope,parent,router,None,b"fixture.invalid",b"fixture-cc").unwrap().unwrap());let a=UserEntryAction::original(owner.clone(),parent.clone(),ImReceiptGate::fresh(),std::time::Instant::now()+std::time::Duration::from_secs(3),b"[phone:+12345@fixture.invalid]").unwrap();ClientUserEntryQuery::original(a,target,router).unwrap()}
fn answer(accepted:bool,aids:&[(&[u8],&[u8])])->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.u32(aids.len()as u32).unwrap();for(uid,aid)in aids{w.bytes(uid).unwrap();w.bytes(b"phone").unwrap();w.bytes(aid).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn batch_result(local:[u32;6],sequence:i32,code:u32,bytes:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();for n in [81,1,1,local[0],local[1],local[2]]{w.u32(n).unwrap();}w.u32(0).unwrap();w.u32(1).unwrap();w.i32(sequence).unwrap();w.u32(code).unwrap();w.bytes(bytes).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn payload(packets:&n::arc_protocol::OwnedCallPackets)->n::rpc_batch::BatchPayload<'_>{assert_eq!(packets.packets.len(),1);let call=decode_request(&packets.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(call.method,b"swapCalls.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");assert_eq!(call.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");let mut r=Reader::versioned(call.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"7_9");assert_eq!(r.bytes().unwrap(),b"fixture-selected");let value=n::rpc_batch::read_payload(&mut r).unwrap();r.finish().unwrap();value}
fn registration()->GatewayRegistration<'static>{GatewayRegistration{authname_prefix:"fixture",channel_id:"fixture",delete_int_code:"1",sip_number:"fixture",sip_password:"fixture"}}
#[test]fn client_read_whole_factory_input_and_cc_are_nested_not_direct_or_account_auth(){let(mut b,who,parent,p)=current();let raw=b"  prefix/UserEntry  ";let q=query(&who,&parent,&p,raw);let sequence=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let data=payload(&packets);let nested=&data.requests[&sequence];assert_eq!(nested.method,b"aidToUid.UserEntry.User");assert_eq!(nested.metadata.len(),2);assert_eq!(nested.metadata.iter().find(|p|p.key==b"id").unwrap().value,raw);assert_eq!(nested.metadata.iter().find(|p|p.key==b"DeviceInfo.CC").unwrap().value,b"fixture-cc");assert!(!nested.metadata.iter().any(|p|matches!(p.key,b"token"|b"pwd"|b"__magic")));assert!(packets.client_only());let mut r=Reader::versioned(nested.arguments).unwrap();assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"phone");assert_eq!(r.bytes().unwrap(),b"+12345");r.finish().unwrap();}
#[test]fn client_read_and_hosting_share_original_positive_nested_sequence_without_outer_id_reuse(){let(mut b,who,parent,p)=current();let first=b.queue_hosting(who.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();let q=query(&who,&parent,&p,b"UserEntry");let read=b.queue_client_user_entry(q).unwrap();let last=b.queue_hosting(who.clone(),"uid:7_9",STATUS_UNREGISTER,&registration(),&[]).unwrap();assert_eq!((first,read,last),(1,2,3));let packets=b.prepare_swap(1,&[]).unwrap();assert_eq!(packets.call_id,2);assert_eq!(payload(&packets).requests.len(),3);assert!(!packets.client_only());}
#[test]fn client_read_direct_envelope_cannot_complete_a_scheduled_nested_sequence(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();assert_eq!(seq,1);assert_eq!(packets.call_id,2);let events=b.on_plaintext_packet(&who,&parent,&packet(seq,0,&answer(true,&[(b"7_9",b"+12345")])),2).unwrap();assert!(events.is_empty());assert!(b.take_client_user_entry_replies().is_empty());assert!(parent.is_open());}
#[test]fn client_read_matched_result_is_private_data_not_hosting_callback_or_future_replay(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let a=q.action().clone();let sequence=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let words=payload(&packets).fields;let events=b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(words,sequence,0,&answer(true,&[(b"7_9",b"+12345")]))),2).unwrap();assert!(!events.iter().any(|e|matches!(e,OfflineEvent::Callback(_)|OfflineEvent::Unhosting(_))));let mut replies=b.take_client_user_entry_replies();assert_eq!(replies.len(),1);let reply=replies.remove(0).result.unwrap();assert_eq!(reply.observe(&a).unwrap().report,"reported");assert_eq!(reply.resolved_uid_for(&a),Some(b"7_9".as_slice()));let packets=b.prepare_swap(3,&[]).unwrap();assert!(!payload(&packets).requests.contains_key(&sequence));assert!(parent.is_open());}
#[test]fn client_read_duplicate_or_retired_nested_result_cannot_retain_reply_twice(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let words=payload(&packets).fields;b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(words,seq,0,&answer(true,&[(b"7_9",b"+12345")]))),2).unwrap();let replies=b.take_client_user_entry_replies();assert_eq!(replies.len(),1);let before=parent.native_rpc_resources();let next=b.prepare_swap(3,&[]).unwrap();let words=payload(&next).fields;b.on_plaintext_packet(&who,&parent,&packet(next.call_id,0,&batch_result(words,seq,0,&answer(true,&[(b"7_9",b"+12345")]))),4).unwrap();assert!(b.take_client_user_entry_replies().is_empty());assert!(parent.native_rpc_resources().1>=before.1);assert!(parent.is_open());}
#[test]fn client_read_negative_reply_cannot_export_uid_and_version_error_is_not_retried(){for(code,accepted)in [(0,false),(0x10000,true)]{let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let a=q.action().clone();let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let words=payload(&packets).fields;b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(words,seq,code,&answer(accepted,&[(b"7_9",b"+12345")]))),2).unwrap();let mut replies=b.take_client_user_entry_replies();let data=replies.remove(0).result.unwrap();assert_eq!(data.observe(&a).unwrap().report,"rejected");assert_eq!(data.resolved_uid_for(&a),None);assert!(!payload(&b.prepare_swap(3,&[]).unwrap()).requests.contains_key(&seq));assert!(parent.is_open());}}
#[test]fn client_read_strict_foreign_aid_and_ambiguous_result_do_not_grant_or_close_sdk(){for(aids,error)in [(vec![(b"7_9".as_slice(),b"+54321".as_slice())],UserEntryError::Protocol),(vec![(b"7_9".as_slice(),b"+12345".as_slice()),(b"8_9".as_slice(),b"+12345".as_slice())],UserEntryError::Ambiguous)]{let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let words=payload(&packets).fields;b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(words,seq,0,&answer(true,&aids))),2).unwrap();let mut replies=b.take_client_user_entry_replies();assert_eq!(replies.remove(0).result.err(),Some(error));assert!(parent.is_open());}}
#[test]fn client_read_retirement_before_encoding_removes_only_read_and_burns_sequence(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q.clone()).unwrap();q.interest().retire();b.retire_client_user_entry(seq);let next=b.queue_hosting(who.clone(),"uid:7_9",STATUS_UNREGISTER,&registration(),&[]).unwrap();assert_eq!(next,seq+1);let packets=b.prepare_swap(1,&[]).unwrap();assert!(!payload(&packets).requests.contains_key(&seq));assert!(payload(&packets).requests.contains_key(&next));assert!(parent.is_open());}
#[test]fn client_read_retirement_during_prepared_mixed_frame_preserves_immutable_snapshot_and_write_epochs(){let(mut b,who,parent,p)=current();let host=b.queue_hosting(who.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q.clone()).unwrap();let batch=b.scheduler.as_mut().unwrap().prepare(1).unwrap();q.interest().retire();b.retire_client_user_entry(seq);assert_eq!(batch.payload().requests.len(),2);assert!(parent.is_open());let packets=b.protocol.prepare_scheduled_swap(&batch,&[]).unwrap();b.scheduler.as_mut().unwrap().commit_submission(batch,1).unwrap();assert_eq!(payload(&packets).requests.len(),2);let next=b.prepare_swap(2,&[]).unwrap();assert!(payload(&next).requests.contains_key(&host));assert!(!payload(&next).requests.contains_key(&seq));assert!(parent.is_open());}
#[test]fn client_read_does_not_weaken_original_hosting_retirement_of_prepared_write(){let(mut b,who,parent,_)=current();let host=b.queue_hosting(who.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();let _batch=b.scheduler.as_mut().unwrap().prepare(1).unwrap();b.retire_hosting(host);assert!(!parent.is_open());}
#[test]fn client_read_retained_packet_keeps_query_auth_and_shared_charge_after_retirement_and_scope_close(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q.clone()).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();drop(q);b.retire_client_user_entry(seq);let retained=parent.native_rpc_resources();assert!(retained.0>=3&&retained.1>0);b.close();assert_eq!(parent.native_rpc_resources(),retained);drop(packets);assert_eq!(parent.native_rpc_resources(),(0,0));}
#[test]fn client_read_foreign_scope_auth_or_retired_gate_cannot_consume_nested_sequence(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");q.action().gate().retire();assert_eq!(b.queue_client_user_entry(q).err(),Some(UserEntryError::Scope));let host=b.queue_hosting(who.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();assert_eq!(host,1);assert!(parent.is_open());}
#[test]fn client_read_global_capacity_is_shared_and_checked_before_auth_copy_or_id(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let mut held=Vec::new();while let Ok(value)=parent.reserve_native_rpc(1){held.push(value);}assert_eq!(parent.native_rpc_resources().0,128);assert_eq!(b.queue_client_user_entry(q).err(),Some(UserEntryError::Capacity));drop(held);let host=b.queue_hosting(who.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();assert_eq!(host,1);assert!(parent.is_open());}
#[test]fn client_read_argument_measure_matches_original_six_words_and_selected_session_encoding(){let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");b.queue_client_user_entry(q).unwrap();let batch=b.scheduler.as_mut().unwrap().prepare(1).unwrap();let args=n::rpc_batch::encode_swap_calls(b"7_9",b"fixture-selected",&batch.payload()).unwrap();assert_eq!(batch.encoded_argument_bytes(3,b"fixture-selected".len()).unwrap(),args.expose_to_transport().len());b.scheduler.as_mut().unwrap().cancel_prepared();assert!(parent.is_open());}
#[test]fn client_read_detached_actual_packets_keep_charge_until_last_packet_drop(){
    let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q).unwrap();let mut packets=b.prepare_swap(1,&[]).unwrap();let detached=std::mem::take(&mut packets.packets);b.retire_client_user_entry(seq);b.close();let original=parent.native_rpc_resources();assert!(original.0>=3&&original.1>0);drop(packets);assert_eq!(parent.native_rpc_resources(),original);assert!(detached.iter().all(|p|!p.expose_to_transport().is_empty()));drop(detached);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]fn client_read_result_capacity_refusal_precedes_raw_result_capture_without_closing_sdk(){
    let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();let words=payload(&packets).fields;let remaining=128*1024-parent.native_rpc_resources().1;let pressure=parent.reserve_native_rpc(remaining).unwrap();let original=parent.native_rpc_resources();
    b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(words,seq,0,&answer(true,&[(b"7_9",b"+12345")]))),2).unwrap();let mut replies=b.take_client_user_entry_replies();assert_eq!(replies.len(),1);assert_eq!(replies.remove(0).result.err(),Some(UserEntryError::Capacity));assert_eq!(parent.native_rpc_resources(),original);assert!(parent.is_open());drop(pressure);b.close();drop(packets);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]fn client_read_retired_between_admission_and_prepare_has_no_business_frame_or_retry(){
    let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let seq=b.queue_client_user_entry(q.clone()).unwrap();q.interest().retire();let packets=b.prepare_swap(1,&[]).unwrap();assert!(payload(&packets).requests.is_empty());assert!(!packets.has_business_work());assert!(!packets.client_only());b.retire_client_user_entry(seq);b.retire_request(packets.call_id);let next=b.queue_hosting(who.clone(),"uid:7_9",STATUS_UNREGISTER,&registration(),&[]).unwrap();assert_eq!(next,seq+1);assert!(parent.is_open());
}

fn ack_without_result(local:[u32;6])->Vec<u8>{
    let mut w=Writer::new();w.boolean(true).unwrap();
    for value in [81,1,1,local[0],local[1],local[2]]{w.u32(value).unwrap();}
    w.u32(0).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();
    w.finish().expose_to_transport().to_vec()
}
#[test]
fn client_read_acked_epoch_before_later_result_keeps_private_identity_and_exact_charge(){
    let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let action=q.action().clone();
    let seq=b.queue_client_user_entry(q).unwrap();let first=b.prepare_swap(1,&[]).unwrap();let words=payload(&first).fields;
    let events=b.on_plaintext_packet(&who,&parent,&packet(first.call_id,0,&ack_without_result(words)),2).unwrap();
    assert!(events.is_empty());assert!(b.take_client_user_entry_replies().is_empty());
    assert_eq!(b.scheduler.as_ref().unwrap().history_count(),0);assert_eq!(b.scheduler.as_ref().unwrap().pending_client_count(),1);
    let next=b.prepare_swap(3,&[]).unwrap();assert!(payload(&next).requests.is_empty());
    let before=parent.native_rpc_resources();let bytes=answer(true,&[(b"7_9",b"+12345")]);
    let events=b.on_plaintext_packet(&who,&parent,&packet(next.call_id,0,&batch_result(payload(&next).fields,seq,0,&bytes)),4).unwrap();
    assert!(events.is_empty());let mut completions=b.take_client_user_entry_replies();assert_eq!(completions.len(),1);
    let data=completions.pop().unwrap().result.unwrap();assert_eq!(data.observe(&action).unwrap().report,"reported");
    assert_eq!(data.resolved_uid_for(&action),Some(b"7_9".as_slice()));
    assert!(parent.native_rpc_resources().1>=before.1+n::user_entry_rpc::client_result_peak(bytes.len()).unwrap());
    let held=parent.native_rpc_resources();let duplicate=b.prepare_swap(5,&[]).unwrap();
    b.on_plaintext_packet(&who,&parent,&packet(duplicate.call_id,0,&batch_result(payload(&duplicate).fields,seq,0,&bytes)),6).unwrap();
    assert!(b.take_client_user_entry_replies().is_empty());assert_eq!(parent.native_rpc_resources(),held);
    assert!(parent.is_open());b.close();drop(first);drop(data);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]
fn client_read_acked_epoch_later_result_capacity_refusal_has_no_generic_copy_or_sdk_close(){
    let(mut b,who,parent,p)=current();let seq=b.queue_client_user_entry(query(&who,&parent,&p,b"UserEntry")).unwrap();
    let first=b.prepare_swap(1,&[]).unwrap();b.on_plaintext_packet(&who,&parent,&packet(first.call_id,0,&ack_without_result(payload(&first).fields)),2).unwrap();
    assert!(b.take_client_user_entry_replies().is_empty());assert_eq!(b.scheduler.as_ref().unwrap().history_count(),0);
    let next=b.prepare_swap(3,&[]).unwrap();assert!(payload(&next).requests.is_empty());
    let pressure=parent.reserve_native_rpc(128*1024-parent.native_rpc_resources().1).unwrap();let full=parent.native_rpc_resources();
    b.on_plaintext_packet(&who,&parent,&packet(next.call_id,0,&batch_result(payload(&next).fields,seq,0,&answer(true,&[(b"7_9",b"+12345")]))),4).unwrap();
    let mut completions=b.take_client_user_entry_replies();assert_eq!(completions.len(),1);assert_eq!(completions.pop().unwrap().result.err(),Some(UserEntryError::Capacity));
    assert_eq!(parent.native_rpc_resources(),full);assert!(parent.is_open());drop(pressure);b.close();drop(first);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]
fn client_read_measured_encoders_keep_exact_canonical_bytes_and_no_full_bound_allocations(){
    let aid=n::user_entry_data::RecipientUri::parse(b"[phone:+12345@fixture.invalid]").unwrap().aid();
    let measured=n::client_user_entry_encoding::encode_aid_arguments(aid).unwrap();let original=n::user_entry_data::encode_aid_to_uid(&[aid]).unwrap();
    assert_eq!(measured.expose_to_transport(),original.expose_to_transport());assert_eq!(measured.allocated_capacity(),measured.expose_to_transport().len());
    assert_eq!(original.allocated_capacity(),128*1024);
    let(mut b,who,parent,p)=current();b.queue_client_user_entry(query(&who,&parent,&p,b"UserEntry")).unwrap();let batch=b.scheduler.as_mut().unwrap().prepare(1).unwrap();
    let shape=batch.payload();let measured=n::client_user_entry_encoding::encode_swap_calls(b"7_9",b"fixture-selected",&shape).unwrap();
    let original=n::rpc_batch::encode_swap_calls(b"7_9",b"fixture-selected",&shape).unwrap();assert_eq!(measured.expose_to_transport(),original.expose_to_transport());
    assert_eq!(measured.allocated_capacity(),measured.expose_to_transport().len());assert_eq!(batch.encoded_argument_bytes(3,b"fixture-selected".len()).unwrap(),measured.allocated_capacity());
    let fields=[n::rpc_stream::RpcProperty{key:b"id",value:b"DestEntry"},n::rpc_stream::RpcProperty{key:b"token",value:b"fixture-auth"}];
    let envelope=n::client_user_entry_encoding::encode_request(17,&fields,b"swapCalls.AccountEntry.Account",measured.expose_to_transport()).unwrap();
    let old=n::rpc_envelope::encode_request(17,&fields,b"swapCalls.AccountEntry.Account",measured.expose_to_transport()).unwrap();
    assert_eq!(envelope.expose_to_transport(),old.expose_to_transport());assert_eq!(envelope.allocated_capacity(),envelope.expose_to_transport().len());
    b.scheduler.as_mut().unwrap().cancel_prepared();assert!(parent.is_open());
}
#[test]
fn client_read_measured_writer_refuses_growth_and_retains_original_codec_rejections(){
    let mut writer=Writer::measured(5).unwrap();assert_eq!(writer.bytes(b"secret").err(),Some(n::rpc_stream::CodecError::TooLarge));
    assert_eq!(writer.finish().allocated_capacity(),5);
    let duplicate=[n::rpc_stream::RpcProperty{key:b"id",value:b"a"},n::rpc_stream::RpcProperty{key:b"id",value:b"b"}];
    for(id,method,fields,error)in [(0,b"method".as_slice(),&[][..],n::rpc_stream::CodecError::InvalidCallId),(1,b"".as_slice(),&[][..],n::rpc_stream::CodecError::InvalidField),(1,b"method".as_slice(),&duplicate[..],n::rpc_stream::CodecError::DuplicateKey)]{
        assert_eq!(n::client_user_entry_encoding::encode_request(id,fields,method,b"arguments").err(),Some(error));
        assert_eq!(n::rpc_envelope::encode_request(id,fields,method,b"arguments").err(),Some(error));
    }
}
#[test]
fn client_read_actual_detached_fragments_refund_only_after_final_packet_payload_drop(){
    let(mut b,who,parent,p)=current();let seq=b.queue_client_user_entry(query(&who,&parent,&p,b"UserEntry")).unwrap();
    let padding=vec![b'p';20_000];let routing=[n::rpc_stream::RpcProperty{key:b"padding",value:&padding}];let mut descriptor=b.prepare_swap(1,&routing).unwrap();
    assert!(descriptor.packets.len()>1);let mut actual=std::mem::take(&mut descriptor.packets);let last=actual.pop().unwrap();
    b.retire_client_user_entry(seq);b.close();let retained=parent.native_rpc_resources();assert!(retained.1>3*20_000&&retained.1<=128*1024);
    drop(descriptor);assert_eq!(parent.native_rpc_resources(),retained);
    while let Some(fragment)=actual.pop(){assert!(!fragment.expose_to_transport().is_empty());drop(fragment);assert_eq!(parent.native_rpc_resources(),retained);}
    drop(actual);assert_eq!(parent.native_rpc_resources(),retained);assert!(!last.expose_to_transport().is_empty());drop(last);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]
fn client_read_packet_byte_pressure_refuses_before_outer_id_and_keeps_original_pending_read(){
    let(mut b,who,parent,p)=current();let seq=b.queue_client_user_entry(query(&who,&parent,&p,b"UserEntry")).unwrap();
    let pressure=parent.reserve_native_rpc(128*1024-parent.native_rpc_resources().1).unwrap();let full=parent.native_rpc_resources();
    assert_eq!(b.prepare_swap(1,&[]).err(),Some(n::arc_protocol::ProtocolError::Batch(n::batch_scheduler::BatchError::Backpressure)));
    assert_eq!(parent.native_rpc_resources(),full);assert!(parent.is_open());assert_eq!(b.scheduler.as_ref().unwrap().pending_client_count(),1);
    drop(pressure);let packets=b.prepare_swap(2,&[]).unwrap();assert_eq!(packets.call_id,2);assert!(payload(&packets).requests.contains_key(&seq));
    b.retire_client_user_entry(seq);b.close();drop(packets);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]
fn client_read_reply_footer_model_and_reply_only_last_holder_remain_charged_after_close(){
    let(mut b,who,parent,p)=current();let q=query(&who,&parent,&p,b"UserEntry");let action=q.action().clone();let seq=b.queue_client_user_entry(q).unwrap();let packets=b.prepare_swap(1,&[]).unwrap();
    let keys=(0..128u8).map(|key|[key]).collect::<Vec<_>>();let fields=keys.iter().map(|key|n::rpc_stream::RpcProperty{key:key.as_slice(),value:b""}).collect::<Vec<_>>();
    let mut w=Writer::new();w.boolean(true).unwrap();w.u32(0).unwrap();w.bytes(b"").unwrap();w.properties(&fields).unwrap();let bytes=w.finish();
    let peak=n::user_entry_rpc::client_result_peak(bytes.expose_to_transport().len()).unwrap();assert!(peak>=2*bytes.expose_to_transport().len()+128*std::mem::size_of::<n::rpc_stream::RpcProperty<'_>>());
    b.on_plaintext_packet(&who,&parent,&packet(packets.call_id,0,&batch_result(payload(&packets).fields,seq,0,bytes.expose_to_transport())),2).unwrap();
    let mut replies=b.take_client_user_entry_replies();assert_eq!(replies.len(),1);let only=Arc::new(replies.pop().unwrap().result.unwrap());let last=only.clone();
    assert_eq!(only.observe(&action).unwrap().report,"unreported");assert_eq!(only.resolved_uid_for(&action),None);
    b.close();drop(packets);let retained=parent.native_rpc_resources();assert!(retained.0>0&&retained.1>=peak);drop(only);assert_eq!(parent.native_rpc_resources(),retained);
    drop(last);assert_eq!(parent.native_rpc_resources(),(0,0));
}
