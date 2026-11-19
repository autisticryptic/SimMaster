//! Actions-only original sender intent/permit metadata and last-holder fences.
//! Pure protocol/SQLite here, with separate actual SDK IO regression cases.
use super::*;
fn digest_message(value:&str)->ImSendMessage{ImSendMessage::components(None,"actual body",&[],&[(b"userData".to_vec(),b"opaque-data".to_vec()),(b"OnlineMsg".to_vec(),b"0".to_vec()),(b"imdnId".to_vec(),b"opaque-id".to_vec())],None).unwrap().with_notification_digest(value).unwrap()}
fn prepare(f:&Fixture,message:ImSendMessage,nonce:&str)->Result<PreparedImSend,&'static str>{ImSendAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_send_identity().unwrap(),b"1_2",message,nonce,Instant::now()+Duration::from_secs(30),&policy())}
#[test]
fn im_send_notification_metadata_digest_and_original_parameter_keys_preserve_body_account_and_opaque_strings(){
    let f=Fixture::new(true);let PreparedImSend::Fresh(a)=prepare(&f,digest_message("display summary"),&"a".repeat(32)).unwrap()else{panic!()};let p=a.permit(&policy()).unwrap();let mut r=Reader::versioned(p.arguments()).unwrap();assert_eq!(r.bytes().unwrap(),b"P2P/1_2");assert_eq!(r.bytes().unwrap(),b"Text");assert_eq!(r.bytes().unwrap(),br#"{"text":"actual body"}"#);assert!(r.properties().unwrap().is_empty());let params=r.properties().unwrap();assert_eq!(params.iter().find(|p|p.key==b"account").unwrap().value,b"[username:fixture@fixture.invalid]");let fields=r.properties().unwrap();assert_eq!(fields.iter().find(|p|p.key==b"Notify.Text").unwrap().value,b"display summary");assert_eq!(fields.iter().find(|p|p.key==b"Notify.ImUserData").unwrap().value,b"opaque-data");assert_eq!(fields.iter().find(|p|p.key==b"OnlineMsg").unwrap().value,b"0");assert_eq!(fields.iter().find(|p|p.key==b"Notify.ImdnId").unwrap().value,b"opaque-id");assert!(!fields.iter().any(|p|p.key==b"Notify.OnlineMsg"));r.finish().unwrap();assert_eq!(state(&f),(0,None,None));assert_eq!(f.count(),0);
}
#[test]
fn im_send_notification_digest_same_nonce_change_conflicts_and_unknown_blocks_new_nonce_without_any_id(){
    let f=Fixture::new(true);let PreparedImSend::Fresh(a)=prepare(&f,digest_message("summary1"),&"a".repeat(32)).unwrap()else{panic!()};assert_eq!(prepare(&f,digest_message("summary2"),&"a".repeat(32)).err(),Some("app_account_im_send_nonce_conflict"));assert_eq!(prepare(&f,digest_message("summary1"),&"b".repeat(32)).err(),Some("app_account_im_send_uncertain"));let PreparedImSend::Replay(v)=prepare(&f,digest_message("summary1"),&"a".repeat(32)).unwrap()else{panic!()};assert_eq!(v.state,"uncertain");assert_eq!(state(&f),(0,None,None));a.cancel();
}
#[test]
fn im_send_notification_missing_digest_keeps_original_legacy_intent_hash_and_replay_metadata(){
    let f=Fixture::new(true);let PreparedImSend::Fresh(a)=prepare(&f,ImSendMessage::text("legacy body").unwrap(),&"a".repeat(32)).unwrap()else{panic!()};
    let mut hash=ring::digest::Context::new(&ring::digest::SHA256);hash.update(b"original-im-send-body-v1\0");for value in [b"Text".as_slice(),b"legacy body"]{hash.update(&(value.len()as u64).to_be_bytes());hash.update(value);}hash.update(&0u64.to_be_bytes());hash.update(&0u64.to_be_bytes());hash.update(&[0]);let body_hash=hash.finish();
    let identity=f.backend.captured_im_binding().unwrap();let mut intent=ring::digest::Context::new(&ring::digest::SHA256);for value in [b"original-user-send-intent-v1\0".as_slice(),identity.identity().as_slice(),b"[username:fixture@fixture.invalid]",b"1_2",body_hash.as_ref()]{intent.update(&(value.len()as u64).to_be_bytes());intent.update(value);}let expected=intent.finish();let saved:Vec<u8>=f.temp.database.with_conn(|c|c.query_row("SELECT intent_hash FROM app_voice_im_outbox",[],|r|r.get(0)).map_err(|_|"legacy hash")).unwrap();assert_eq!(saved.as_slice(),expected.as_ref());
    let PreparedImSend::Replay(view)=prepare(&f,ImSendMessage::text("legacy body").unwrap(),&"a".repeat(32)).unwrap()else{panic!()};assert_eq!(view.state,"uncertain");assert_eq!(prepare(&f,ImSendMessage::text("legacy body").unwrap().with_notification_digest("").unwrap(),&"a".repeat(32)).err(),Some("app_account_im_send_nonce_conflict"));a.cancel();
}
#[test]
fn im_send_notification_digest_invalid_sources_cannot_create_business_intent_or_policy(){
    let f=Fixture::new(true);let overlong="x".repeat(65537);for bad in ["a\0b",overlong.as_str()]{assert!(ImSendMessage::text("body").unwrap().with_notification_digest(bad).is_err());}
    assert!(ImSendMessage::info("Subtype","body").unwrap().with_notification_digest("digest").is_err());assert!(ImSendMessage::components(None,"body",&[],&[],Some(&[])).unwrap().with_notification_digest("digest").is_err());let count:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_outbox",[],|r|r.get(0)).map_err(|_|"intent count")).unwrap();assert_eq!(count,0);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert!(f.scope.is_open());
}
#[test]
fn im_send_notification_legacy_large_userdata_unknown_row_is_metadata_replay_not_new_permit_or_query(){
    let f=Fixture::new(true);let value=vec![b'a';65536];let message=||ImSendMessage::components(None,"body",&[],&[(b"userData".to_vec(),value.clone())],None).unwrap();
    let PreparedImSend::Fresh(a)=prepare(&f,message(),&"a".repeat(32)).unwrap()else{panic!()};a.cancel();drop(a);
    // Retained historical journal DATA from an earlier supported wire shape;
    // this UPDATE is NOT a native capability, receipt or proof of new IO.
    f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_im_outbox SET state=0,request_id=2",[]).map_err(|_|"historical unknown DATA")).unwrap();
    assert_eq!(f.scope.native_rpc_resources(),(0,0));
    let PreparedImSend::Replay(v)=prepare(&f,message(),&"a".repeat(32)).unwrap()else{panic!("exact legacy context remains metadata-only")};assert_eq!(v.state,"uncertain");assert_eq!(state(&f),(0,Some(2),None));assert_eq!(f.scope.native_rpc_resources(),(0,0));
    assert_eq!(prepare(&f,message(),&"b".repeat(32)).err(),Some("app_account_im_send_uncertain"));assert!(f.scope.is_open());
}
#[test]
fn im_send_notification_digest_retained_permit_stays_charged_after_queued_cancel_until_last_holder(){
    let f=Fixture::new(true);let PreparedImSend::Fresh(a)=prepare(&f,digest_message("private display digest"),&"a".repeat(32)).unwrap()else{panic!()};let p=a.permit(&policy()).unwrap();let cost=f.scope.native_rpc_resources();assert_eq!(cost.0,3);assert!(cost.1>"private display digest".len());p.cancel();assert_eq!(state(&f),(3,None,None));assert_eq!(f.scope.native_rpc_resources(),cost);drop(a);assert_eq!(f.scope.native_rpc_resources(),cost);drop(p);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert!(f.scope.is_open());
}
