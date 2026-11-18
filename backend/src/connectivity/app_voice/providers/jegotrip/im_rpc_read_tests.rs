//! Actions-only original ARC #Group read request/reply ownership. No current
//! actor/consumer, device/SDK execution, remote write or cursor grant.
use super::*;
use n::{rpc_im::{ImQuery,ImReplyData},im_status,im_data::ImBox};
#[path="native_rpc_budget_im_tests.rs"]
mod shared_budget;
fn refresh_result(time:i64,accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.u32(0).unwrap();w.i64(time).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn receive_result(indices:&[i64])->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(indices.len()as u32).unwrap();for index in indices{w.i64(*index).unwrap();w.i64(7).unwrap();w.bytes(b"private-sender").unwrap();w.bytes(b"Text").unwrap();w.bytes(br#"{"text":"received privately"}"#).unwrap();w.properties(&[]).unwrap();w.properties(&[]).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn refresh()->Arc<ImQuery>{Arc::new(ImQuery::refresh(&[],-1).unwrap())}
fn request(f:&mut Fixture,query:Arc<ImQuery>)->n::arc_protocol::OwnedCallPackets{f.backend.prepare_im_read(query,f.gate.clone(),Instant::now()+Duration::from_secs(30)).unwrap()}
fn reply(f:&mut Fixture,packet:&n::arc_protocol::OwnedCallPackets,body:&[u8],time:u64){assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(packet.call_id,body),time).unwrap().is_empty());}
#[test]
fn im_rpc_read_default_absence_cannot_create_a_group_request_or_replace_incoming_configuration(){
    let mut f=Fixture::new(false);assert!(f.backend.prepare_im_read(refresh(),f.gate.clone(),Instant::now()+Duration::from_secs(1)).is_err());assert!(f.backend.take_im_replies().is_empty());assert!(f.scope.is_open());assert_eq!(f.count(),0);
}
#[test]
fn im_rpc_refresh_uses_original_group_oid_native_token_and_actual_uid_not_phone_or_caller_metadata(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());let decoded=decode_request(&p.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(decoded.metadata.len(),2);assert_eq!(decoded.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert_eq!(decoded.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert_eq!(decoded.method,im_status::REFRESH_METHOD.as_bytes());
    let mut r=Reader::versioned(decoded.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),5);for key in [b"ML".as_slice(),b"MLB",b"P:P_MR",b"P:P_MV-N",b"P:P_UDMR-N"]{assert_eq!(r.bytes().unwrap(),key);}assert_eq!(r.i64().unwrap(),-1);r.finish().unwrap();assert_eq!(f.count(),0);
}
#[test]
fn im_rpc_only_matched_original_reply_can_construct_owned_refresh_data_without_hosting_or_sms(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());reply(&mut f,&p,&refresh_result(4294967297,true),1);let owned=f.backend.take_im_replies();assert_eq!(owned.len(),1);assert!(owned[0].matches(&f.owner,&f.scope));assert_eq!(owned[0].request().call_id(),p.call_id);let ImReplyData::Refresh(data)=owned[0].result().unwrap()else{panic!("original refresh kind")};assert!(data.accepted);assert_eq!(data.update_time,4294967297);assert!(f.backend.take_im_replies().is_empty());assert_eq!(f.count(),0);assert_eq!(f.backend.phase(),ArcPhase::Authenticated);
}
#[test]
fn im_rpc_receive_preserves_exact_box_start_count_and_checks_original_requested_range(){
    let mut f=Fixture::new(true);let q=Arc::new(ImQuery::receive(ImBox::parse(b"Org/1-2").unwrap(),4294967297,2).unwrap());let p=request(&mut f,q);let decoded=decode_request(&p.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(decoded.method,n::im_data::RECV_METHOD.as_bytes());let mut r=Reader::versioned(decoded.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"Org/1-2");assert_eq!(r.i64().unwrap(),4294967297);assert_eq!(r.u32().unwrap(),2);r.finish().unwrap();reply(&mut f,&p,&receive_result(&[4294967297,4294967298]),1);let owned=f.backend.take_im_replies();let ImReplyData::Receive(data)=owned[0].result().unwrap()else{panic!("receive")};assert_eq!(data.messages.len(),2);assert_eq!(data.messages[1].index,4294967298);assert_eq!(f.count(),0);
}
#[test]
fn im_rpc_raw_received_messages_are_not_local_sql_consumption_or_original_notification_completions(){
    let mut f=Fixture::new(true);let p=request(&mut f,Arc::new(ImQuery::receive(ImBox::parse(b"P2P/1_2").unwrap(),7,1).unwrap()));reply(&mut f,&p,&receive_result(&[7]),1);assert_eq!(f.backend.take_im_replies().len(),1);let notice=f.hint(91,99,b"P2P/1_2",7);assert!(f.inbox(&notice).lookup(&f.temp.database,&notice).unwrap().is_none());assert_eq!(f.count(),0);assert_eq!(result(f.backend.complete_im(notice.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_rpc_receive_rejects_duplicate_out_of_range_or_over_count_before_owned_data_escapes(){
    for indices in [&[7,7][..],&[7,9],&[6],&[7,8,9]]{let mut f=Fixture::new(true);let p=request(&mut f,Arc::new(ImQuery::receive(ImBox::parse(b"P2P/1_2").unwrap(),7,2).unwrap()));assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&receive_result(indices)),1).is_err());assert!(!f.scope.is_open());assert!(f.backend.take_im_replies().is_empty());assert_eq!(f.count(),0);}
}
#[test]
fn im_rpc_unknown_ids_and_duplicate_original_replies_never_construct_another_owned_result(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id+99,&refresh_result(1,true)),1).unwrap().is_empty());assert!(f.backend.take_im_replies().is_empty());reply(&mut f,&p,&refresh_result(1,true),2);reply(&mut f,&p,&refresh_result(1,true),3);assert_eq!(f.backend.take_im_replies().len(),1);
}
#[test]
fn im_rpc_foreign_owner_or_scope_cannot_consume_an_identical_original_request_id(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());let foreign=Fixture::new(true);let raw=reply_packet(p.call_id,&refresh_result(1,true));assert!(f.backend.on_plaintext_packet(&foreign.owner,&f.scope,&raw,1).unwrap().is_empty());assert!(f.backend.on_plaintext_packet(&f.owner,&foreign.scope,&raw,2).unwrap().is_empty());assert!(f.backend.take_im_replies().is_empty());reply(&mut f,&p,&refresh_result(1,true),3);assert_eq!(f.backend.take_im_replies().len(),1);
}
#[test]
fn im_rpc_parent_gate_retirement_invalidates_queued_reply_and_denies_new_request_without_rebinding(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());reply(&mut f,&p,&refresh_result(1,true),1);let owned=f.backend.take_im_replies();f.gate.retire();assert!(!owned[0].matches(&f.owner,&f.scope));assert!(owned[0].result().is_err());assert!(f.backend.prepare_im_read(refresh(),f.gate.clone(),Instant::now()+Duration::from_secs(1)).is_err());assert_eq!(f.count(),0);
}
#[test]
fn im_rpc_expired_admission_or_revoke_prevents_late_result_and_never_renews_its_original_deadline(){
    let mut f=Fixture::new(true);assert!(f.backend.prepare_im_read(refresh(),f.gate.clone(),Instant::now()).is_err());let p=request(&mut f,refresh());f.scope.mark_session_revoked();assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&refresh_result(1,true)),1).unwrap().is_empty());assert!(f.backend.take_im_replies().is_empty());
}
#[test]
fn im_rpc_setreceived_write_is_explicitly_denied_until_a_separate_durable_cursor_permission_exists(){
    let mut f=Fixture::new(true);let write=Arc::new(ImQuery::set_received(ImBox::parse(b"P2P/1_2").unwrap(),7).unwrap());assert!(f.backend.prepare_im_read(write,f.gate.clone(),Instant::now()+Duration::from_secs(1)).is_err());let read=request(&mut f,refresh());assert_eq!(read.call_id,2);assert!(f.scope.is_open());assert_eq!(f.count(),0);
}
#[test]
fn im_rpc_negative_refresh_is_data_not_failure_reconnect_or_a_positive_status_grant(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());reply(&mut f,&p,&refresh_result(1,false),1);let owned=f.backend.take_im_replies();let ImReplyData::Refresh(result)=owned[0].result().unwrap()else{panic!("refresh")};assert!(!result.accepted);assert!(f.scope.is_open());assert_eq!(f.backend.phase(),ArcPhase::Authenticated);
}
#[test]
fn im_rpc_original_logout_drops_queued_read_results_and_private_debug_never_exports_credentials(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());reply(&mut f,&p,&refresh_result(1,true),1);let owned=f.backend.take_im_replies();let text=format!("{:?} {:?}",owned[0],owned[0].request());for private in ["fixture-auth","fixture-user","#Group"]{assert!(!text.contains(private));}let p=request(&mut f,refresh());reply(&mut f,&p,&refresh_result(2,true),2);f.backend.prepare_logout(&[]).unwrap();assert!(f.backend.take_im_replies().is_empty());
}
