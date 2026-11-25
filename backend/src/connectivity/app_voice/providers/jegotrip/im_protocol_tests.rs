//! Actions-only original IM protocol/SQLite ownership; no device or SDK loads.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{im_delivery::{NativeImConfig,IncomingImDelivery,ImReplySource},im_inbox::{ImInboxScope,ImReceiptGate},native_receiver::{NativeReceiverConfig,NativeReceiverPolicy,NativeReceiverSetup},
    account_test_support::TempDatabase,account_runtime::local_nonce,account_runs,account_store};
use std::{sync::Arc,time::{Duration,Instant}};
#[path="im_association_tests.rs"]
mod associations;
#[path="im_rpc_read_tests.rs"]
mod rpc_reads;
#[path="im_cursor_tests.rs"]
mod cursors;
#[path="im_status_protocol_tests.rs"]
mod status_consumer;
#[path="im_received_protocol_tests.rs"]
mod received_writes;
#[path="im_user_read_protocol_tests.rs"]
mod user_reads;
#[path="im_send_protocol_tests.rs"]
mod sends;
#[path="im_info_runtime_protocol_tests.rs"]
mod info;
#[path="group_directory_protocol_tests.rs"]
mod directory;
#[path="relation_protocol_tests.rs"]
mod relations;
#[path="group_event_protocol_tests.rs"]
mod group_events;
#[path="org_relation_protocol_tests.rs"]
mod org_relations;
#[path="relation_roster_tests.rs"]
mod roster;
struct Fixture{owner:AttemptOwner,scope:RpcConnectionScope,backend:OfflineBackend,temp:TempDatabase,run:Arc<account_runs::RunToken>,gate:Arc<ImReceiptGate>}
impl Fixture{
    fn new(enabled:bool)->Self{Self::with_info(enabled,false)}
    fn with_info(enabled:bool,receive_info:bool)->Self{Self::with_directory(enabled,receive_info,false)}
    fn with_directory(enabled:bool,receive_info:bool,directory_queries:bool)->Self{
        let temp=TempDatabase::new();let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let line="app-account-0123456789abcdef0123456789abcdef";
        let cfg=NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:enabled.then_some(NativeImConfig{notification_timeout_ms:30000,receive_info,directory_queries})};
        let mut config=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",1);config.profile.incoming=None;config.profile.client_ex=true;config.profile.native_receivers=Some(cfg.clone());config.credentials.sdk.expires_at_unix_seconds=None;
        temp.database.with_conn(|conn|account_store::create(conn,line,&config,100)).unwrap();let run=Arc::new(temp.database.with_conn(|conn|account_runs::register_runtime(conn,line,1,&local_nonce().unwrap())).unwrap());
        let mut session=HostingSession::new(line.into());session.import(imported(),100).unwrap();let owner=session.begin_login(100).unwrap();let mut backend=OfflineBackend::new(owner.clone());backend.claim_common_transport().unwrap();backend.bind_common_identity(b"AccountEntry").unwrap();let scope=backend.connection_scope();
        backend.arm_native_receivers(NativeReceiverSetup::capture(NativeReceiverPolicy::capture(&cfg).unwrap(),true,None).unwrap()).unwrap();let request=backend.prepare_login(login(),&[]).unwrap();backend.on_plaintext_packet(&owner,&scope,&reply_packet(request.call_id,&login_result()),0).unwrap();backend.configure_native_receivers(7).unwrap();
        Self{owner,scope,backend,temp,run,gate:ImReceiptGate::fresh()}
    }
    fn receive(&mut self,rpc:i32,method:&[u8],arguments:&[u8],token:Option<&[u8]>)->Vec<OfflineEvent>{
        let magic=rpc.to_be_bytes();let mut fields=vec![RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"__magic",value:&magic}];if let Some(token)=token{fields.push(RpcProperty{key:b"token",value:token});}
        let raw=encode_request(rpc,&fields,method,arguments).unwrap();let mut p=vec![0];p.extend_from_slice(raw.expose_to_transport());self.backend.on_plaintext_packet(&self.owner,&self.scope,&p,rpc as u64).unwrap()
    }
    fn message(&mut self,rpc:i32,box_id:&[u8],index:i64,text:&str)->Box<IncomingImDelivery>{take(self.receive(rpc,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&more(box_id,index,b"Text",text.as_bytes()),None))}
    fn hint(&mut self,rpc:i32,id:u64,box_id:&[u8],index:i64)->Box<IncomingImDelivery>{take(self.receive(rpc,NOTIFY_CALLS_METHOD.as_bytes(),&hint(id,box_id,index),Some(b"fixture-auth")))}
    fn inbox(&self,delivery:&IncomingImDelivery)->ImInboxScope{ImInboxScope::original(self.run.clone(),delivery.notice().binding().clone(),self.gate.clone()).unwrap()}
    fn count(&self)->i64{self.temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_im_messages",[],|row|row.get(0)).map_err(|_|"fixture")).unwrap()}
}
fn take(events:Vec<OfflineEvent>)->Box<IncomingImDelivery>{assert_eq!(events.len(),1);match events.into_iter().next().unwrap(){OfflineEvent::Im(delivery)=>delivery,_=>panic!("only original deferred IM delivery")}}
fn more(box_id:&[u8],index:i64,kind:&[u8],content:&[u8])->Vec<u8>{
    let mut raw=Writer::new();raw.i64(index).unwrap();raw.i64(-123).unwrap();raw.bytes(b"private-sender-uid").unwrap();raw.bytes(kind).unwrap();raw.bytes(content).unwrap();raw.properties(&[RpcProperty{key:b"../private-attachment",value:b"private-attachment-bytes"}]).unwrap();raw.properties(&[RpcProperty{key:b"account",value:b"private-native-account"}]).unwrap();
    let mut w=Writer::versioned().unwrap();w.bytes(n::im_data::MORE_MESSAGE_KIND).unwrap();w.properties(&[RpcProperty{key:b"Box",value:box_id}]).unwrap();w.bytes(raw.finish().expose_to_transport()).unwrap();w.finish().expose_to_transport().to_vec()
}
fn hint(id:u64,box_id:&[u8],index:i64)->Vec<u8>{let index=index.to_string();let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();w.properties(&[RpcProperty{key:b"Notify.Command",value:b"Message"},RpcProperty{key:b"Notify.Box",value:box_id},RpcProperty{key:b"Notify.MsgIdx",value:index.as_bytes()}]).unwrap();w.finish().expose_to_transport().to_vec()}
fn result(completion:Option<n::offline_backend::CallCompletion>)->Vec<u8>{let Some(n::offline_backend::CallCompletion::Direct(p))=completion else{panic!("direct original result")};decode_reply(&p.packets[0].expose_to_transport()[1..]).unwrap().result_stream.to_vec()}
#[test]
fn im_runtime_default_absence_retains_original_moremessage_and_known_notification_nacks(){
    let mut f=Fixture::new(false);let message=f.receive(1,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&more(b"P2P/1_2",7,b"Text",br#"{"text":"hello"}"#),None);assert_eq!(control_result(&message),[0;6]);let notification=f.receive(2,NOTIFY_CALLS_METHOD.as_bytes(),&hint(99,b"P2P/1_2",7),Some(b"fixture-auth"));assert_eq!(control_result(&notification),[0;6]);assert_eq!(f.count(),0);assert!(f.scope.is_open());
}
#[test]
fn im_runtime_moremessage_keeps_original_ticket_and_cannot_create_sms_call_hint_or_im_before_commit(){
    let mut f=Fixture::new(true);let message=f.message(1,b"Org/1-2",0x1_2345_6789,r#"{"text":"hello"}"#);assert!(message.matches(&f.owner,&f.scope));assert_eq!(f.count(),0);assert!(message.notice().notification_id().is_none());assert!(matches!(message.ticket().source(),ImReplySource::Direct(_)));assert_eq!(message.notice().message().unwrap().unwrap().index,0x1_2345_6789);assert!(f.temp.database.get_sms_messages_for_channel(10,0,None,Some(f.run.line_id())).unwrap().is_empty());assert!(f.temp.database.get_call_history_for_line(f.run.line_id(),10,0).unwrap().is_empty());
}
#[test]
fn im_runtime_durable_text_and_event_commit_before_original_moremessage_reply(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/0_1",7,r#"{"text":"hello 文本","userData":"private-user-data"}"#);let scope=f.inbox(&message);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();assert_eq!(f.count(),1);assert!(proof.authorizes(message.ticket(),Instant::now()));
    let rows=f.temp.database.with_conn(|conn|n::im_inbox::list(conn,f.run.line_id(),16)).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].text,"hello 文本");assert_eq!(rows[0].message_index,"7");assert_eq!(rows[0].attachment_count,1);
    let serialized=serde_json::to_string(&rows).unwrap();for private in ["private-user-data","private-native-account","private-sender-uid","private-attachment","identity_key","fingerprint","box_id"]{assert!(!serialized.contains(private));}
    let events=f.temp.database.get_recent_app_events(Some(f.run.line_id()),10).unwrap();assert_eq!(events.len(),1);assert_eq!(events[0].event_type,"im.received");assert_eq!(result(f.backend.complete_im(message.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
}
#[test]
fn im_runtime_notification_before_message_stays_pending_until_exact_box_and_index_are_consumed(){
    let mut f=Fixture::new(true);let notification=f.hint(1,0xf123_4567_89ab_cdef,b"P2P/1_2",7);assert!(f.inbox(&notification).lookup(&f.temp.database,&notification).unwrap().is_none());
    let wrong=f.message(2,b"Org/1-2",7,r#"{"text":"other box"}"#);let proof=f.inbox(&wrong).commit(&f.temp.database,&wrong).unwrap().unwrap();assert!(!proof.authorizes(notification.ticket(),Instant::now()));
    let right=f.message(3,b"P2P/1_2",7,r#"{"text":"right box"}"#);let proof=f.inbox(&right).commit(&f.temp.database,&right).unwrap().unwrap();assert!(proof.authorizes(notification.ticket(),Instant::now()));assert_eq!(result(f.backend.complete_im(notification.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
}
#[test]
fn im_runtime_later_notification_looks_up_durable_original_context_without_duplicate_history_or_event(){
    let mut f=Fixture::new(true);let message=f.message(1,b"Org/1+2",19,r#"{"text":"once"}"#);f.inbox(&message).commit(&f.temp.database,&message).unwrap();let notification=f.hint(2,33,b"Org/1+2",19);let proof=f.inbox(&notification).lookup(&f.temp.database,&notification).unwrap().unwrap();assert!(proof.authorizes(notification.ticket(),Instant::now()));assert_eq!(f.count(),1);assert_eq!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),10).unwrap().len(),1);
}
#[test]
fn im_runtime_duplicate_moremessage_is_idempotent_but_changed_same_index_is_refused(){
    let mut f=Fixture::new(true);let first=f.message(1,b"P2P/1_2",7,r#"{"text":"one"}"#);f.inbox(&first).commit(&f.temp.database,&first).unwrap();let duplicate=f.message(2,b"P2P/1_2",7,r#"{"text":"one"}"#);assert!(f.inbox(&duplicate).commit(&f.temp.database,&duplicate).unwrap().is_some());let changed=f.message(3,b"P2P/1_2",7,r#"{"text":"two"}"#);assert_eq!(f.inbox(&changed).commit(&f.temp.database,&changed).err(),Some("app_account_im_message_conflict"));assert_eq!(f.count(),1);assert_eq!(result(f.backend.complete_im(changed.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_runtime_deleted_message_becomes_receipt_tombstone_and_replays_never_restore_text(){
    let mut f=Fixture::new(true);let first=f.message(1,b"P2P/1_2",7,r#"{"text":"deleted private text"}"#);f.inbox(&first).commit(&f.temp.database,&first).unwrap();let id=f.temp.database.with_conn(|conn|n::im_inbox::list(conn,f.run.line_id(),16)).unwrap()[0].id;
    assert!(f.temp.database.with_conn(|conn|n::im_inbox::delete(conn,f.run.line_id(),id)).unwrap());let replay=f.message(2,b"P2P/1_2",7,r#"{"text":"deleted private text"}"#);assert!(f.inbox(&replay).commit(&f.temp.database,&replay).unwrap().is_some());let notification=f.hint(3,99,b"P2P/1_2",7);assert!(f.inbox(&notification).lookup(&f.temp.database,&notification).unwrap().is_some());
    assert!(f.temp.database.with_conn(|conn|n::im_inbox::list(conn,f.run.line_id(),16)).unwrap().is_empty());assert_eq!(f.count(),1);let retained:i64=f.temp.database.with_conn(|conn|conn.query_row("SELECT length(body)+length(text)+length(sender) FROM app_voice_im_messages",[],|row|row.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(retained,0);
}
#[test]
fn im_runtime_unknown_kind_or_malformed_text_never_creates_consumption_proof(){
    let mut f=Fixture::new(true);let unknown=take(f.receive(1,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&more(b"P2P/1_2",7,b"Info.Unknown",b"not text"),None));assert!(f.inbox(&unknown).commit(&f.temp.database,&unknown).unwrap().is_none());let invalid=f.message(2,b"P2P/1_2",7,"not-json");assert!(f.inbox(&invalid).commit(&f.temp.database,&invalid).is_err());assert_eq!(f.count(),0);assert_eq!(result(f.backend.complete_im(invalid.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_runtime_sql_event_failure_rolls_back_message_before_any_positive_reply(){
    let mut f=Fixture::new(true);f.temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_im BEFORE INSERT ON app_events WHEN NEW.event_type='im.received' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();let message=f.message(1,b"P2P/1_2",7,r#"{"text":"not committed"}"#);assert!(f.inbox(&message).commit(&f.temp.database,&message).is_err());assert_eq!(f.count(),0);assert_eq!(result(f.backend.complete_im(message.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_runtime_original_revision_and_live_runtime_lease_are_required_for_commit_and_lookup(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"revision"}"#);let inbox=f.inbox(&message);f.temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2",[]).map_err(|_|"fixture")).unwrap();assert!(inbox.commit(&f.temp.database,&message).is_err());assert_eq!(f.count(),0);
}
#[test]
fn im_runtime_foreign_scope_cannot_redeem_an_identical_key_or_row_receipt(){
    let mut first=Fixture::new(true);let mut other=Fixture::new(true);let message=first.message(1,b"P2P/1_2",7,r#"{"text":"same"}"#);let proof=first.inbox(&message).commit(&first.temp.database,&message).unwrap().unwrap();let foreign=other.message(1,b"P2P/1_2",7,r#"{"text":"same"}"#);assert!(!proof.authorizes(foreign.ticket(),Instant::now()));assert!(other.backend.complete_im(foreign.ticket(),Some(&proof)).is_err());assert_eq!(other.count(),0);
}
#[test]
fn im_runtime_parent_stop_gate_revokes_persisted_permit_before_io_without_closing_other_layers_itself(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"stop"}"#);let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();f.gate.retire();assert!(!proof.authorizes(message.ticket(),Instant::now()));assert!(f.backend.complete_im(message.ticket(),Some(&proof)).is_err());assert!(f.scope.is_open());assert_eq!(f.count(),1);
}
#[test]
fn im_runtime_scope_close_and_session_revocation_prevent_late_positive_receipts(){
    for revoked in [false,true]{let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"scope"}"#);let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();if revoked{f.scope.mark_session_revoked();}else{f.scope.close();}assert!(!proof.authorizes(message.ticket(),Instant::now()));assert!(f.backend.complete_im(message.ticket(),Some(&proof)).unwrap().is_none());}
}
#[test]
fn im_runtime_expired_or_clipped_tickets_cannot_be_revived_by_persistent_data(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"time"}"#);let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();let expired=message.ticket().clipped_to(Instant::now());assert!(!proof.authorizes(&expired,Instant::now()));let extended=expired.clipped_to(Instant::now()+Duration::from_secs(60));assert_eq!(extended.completion_deadline(),expired.completion_deadline());assert!(f.backend.complete_im(&extended,Some(&proof)).is_err());assert_eq!(result(f.backend.complete_im(&expired,None).unwrap()),[0;6]);
}
#[test]
fn im_runtime_notification_still_needs_original_native_token_not_phone_or_missing_values(){
    let mut f=Fixture::new(true);for(rpc,token)in[(1,None),(2,Some(b"fixture-phone".as_slice())),(3,Some(b"fixture-auth-extra"))]{let events=f.receive(rpc,NOTIFY_CALLS_METHOD.as_bytes(),&hint(99,b"P2P/1_2",7),token);assert_eq!(control_result(&events),[0;6]);assert_eq!(events.len(),1);}assert_eq!(f.count(),0);assert!(f.scope.is_open());
}
#[test]
fn im_runtime_im_ticket_cannot_be_completed_through_generic_sms_path(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"not SMS"}"#);let ImReplySource::Direct(ticket)=message.ticket().source()else{panic!("direct")};assert!(f.backend.complete_direct_message(ticket,true).is_err());assert_eq!(f.count(),0);assert_eq!(result(f.backend.complete_im(message.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_runtime_configuration_is_explicit_bounded_and_absence_is_omitted_from_private_bytes(){
    let f=Fixture::new(false);let stored=f.temp.database.with_conn(|conn|account_store::load(conn,f.run.line_id())).unwrap().unwrap();let bytes=stored.config.encode_private().unwrap();assert!(!std::str::from_utf8(&bytes).unwrap().contains("im_messages"));let value:NativeReceiverConfig=serde_json::from_str("{}").unwrap();assert!(value.im_messages.is_none());for ms in [0,99,30001,u64::MAX]{assert!(NativeImConfig{notification_timeout_ms:ms,receive_info:false,directory_queries:false}.timeout().is_err());}
}
#[test]
fn im_runtime_full_signed_message_indices_stay_exact_in_sql_and_public_strings(){
    let mut f=Fixture::new(true);for(rpc,index)in[(1,i64::MIN),(2,-1),(3,i64::MAX)]{let message=f.message(rpc,b"P2P/1_2",index,r#"{"text":"index"}"#);f.inbox(&message).commit(&f.temp.database,&message).unwrap();}let rows=f.temp.database.with_conn(|conn|n::im_inbox::list(conn,f.run.line_id(),16)).unwrap();assert_eq!(rows.iter().map(|row|row.message_index.as_str()).collect::<Vec<_>>(),["9223372036854775807","-1","-9223372036854775808"]);
}
fn batch(f:&mut Fixture,method:&[u8],arguments:&[u8])->Box<IncomingImDelivery>{
    let request=f.backend.prepare_swap(0,&[]).unwrap();let outer=decode_request(&request.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{w.u32(word).unwrap();}w.u32(1).unwrap();w.i32(1).unwrap();w.bytes(method).unwrap();w.bytes(arguments).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"}]).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();
    let events=f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,w.finish().expose_to_transport()),1).unwrap();assert!(!events.iter().any(|event|matches!(event,OfflineEvent::Outgoing(_)|OfflineEvent::ClientEx(_)|OfflineEvent::Message(_))));events.into_iter().find_map(|event|if let OfflineEvent::Im(delivery)=event{Some(delivery)}else{None}).unwrap()
}
#[test]
fn im_runtime_batch_cannot_use_sms_completion_and_retains_own_result_until_acknowledged_epoch(){
    let mut f=Fixture::new(true);let message=batch(&mut f,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&more(b"P2P/1_2",7,b"Text",br#"{"text":"batch"}"#));let ImReplySource::Batch(inner)=message.ticket().source()else{panic!("batch")};assert!(f.backend.complete_message(inner,true).is_err());let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();assert!(matches!(f.backend.complete_im(message.ticket(),Some(&proof)).unwrap(),Some(n::offline_backend::CallCompletion::BatchQueued)));
    assert_eq!(f.backend.im_write_guard().0.len(),1);let next=f.backend.prepare_swap(2,&[]).unwrap();let outer=decode_request(&next.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();assert_eq!(payload.results[&1].result_stream,[0,1,0,0,0,0]);let ack=n::runtime_backend::tests::swap_result(None,true,payload.fields);f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,&ack),3).unwrap();assert!(f.backend.im_write_guard().0.is_empty());
}
#[test]
fn im_runtime_batch_notification_uses_persistent_exact_key_not_raw_bool_or_poll_hint(){
    let mut f=Fixture::new(true);let message=f.message(1,b"Org/1-2",7,r#"{"text":"before hint"}"#);let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();f.backend.complete_im(message.ticket(),Some(&proof)).unwrap();let notification=batch(&mut f,NOTIFY_CALLS_METHOD.as_bytes(),&hint(0x1_0000_0001,b"Org/1-2",7));let receipt=f.inbox(&notification).lookup(&f.temp.database,&notification).unwrap().unwrap();assert!(receipt.authorizes(notification.ticket(),Instant::now()));assert!(matches!(f.backend.complete_im(notification.ticket(),Some(&receipt)).unwrap(),Some(n::offline_backend::CallCompletion::BatchQueued)));assert_eq!(f.count(),1);
}
#[test]
fn im_runtime_retired_batch_receipt_is_not_replayed_or_changed_to_a_new_positive(){
    let mut f=Fixture::new(true);let message=batch(&mut f,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&more(b"P2P/1_2",7,b"Text",br#"{"text":"retire"}"#));let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();f.backend.complete_im(message.ticket(),Some(&proof)).unwrap();f.gate.retire();assert!(f.backend.prepare_swap(2,&[]).is_err());assert!(!f.scope.is_open());assert_eq!(f.count(),1);
}
#[test]
fn im_runtime_private_debug_never_reflects_sender_body_attachment_or_native_identity(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"private-content"}"#);let scope=f.inbox(&message);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();let printed=format!("{message:?} {:?} {:?} {scope:?} {proof:?}",message.ticket(),message.notice());for private in ["private-content","P2P/1_2","fixture","sender","attachment","identity_key"]{assert!(!printed.contains(private));}
}

