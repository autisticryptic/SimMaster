//! Actions-only local body/nonce/intent/SQL/data boundaries. No decoder bool or
//! local UID data can supply the required owned network recipient result.
use super::*;
fn recipient_action(f:&Fixture,id:i64,nonce:&str,recipient:&[u8])->Arc<UserReadAction>{UserReadAction::original_with_recipient(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),id,nonce,Instant::now()+Duration::from_secs(30),recipient).unwrap()}
fn uri()->&'static[u8]{b"[phone:+12345@fixture.invalid]"}
fn hash_uri(uri:&[u8])->Vec<u8>{let mut h=ring::digest::Context::new(&ring::digest::SHA256);h.update(b"original-im-user-read-recipient-v1\0");h.update(uri);h.finish().as_ref().to_vec()}
#[test]
fn user_read_recipient_only_explicit_typed_nonuid_is_a_valid_intent_not_uid_or_naked_phone(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);for u in [b"7_9".as_slice(),b"[uid:7_9]",b"12345",b"scheme://fixture.invalid"]{assert!(UserReadAction::original_with_recipient(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),id,&"a".repeat(32),Instant::now()+Duration::from_secs(1),u).is_err());}let a=recipient_action(&f,id,&"a".repeat(32),uri());assert_eq!(a.recipient(),Some(uri()));assert!(f.scope.is_open());
}
#[test]
fn user_read_recipient_matching_group_status_alone_cannot_mint_an_alias_write_permit(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=recipient_action(&f,id,&"a".repeat(32),uri());let BoundUserRead::Context(c)=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!()};let p=f.backend.prepare_im_read(c.query(),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&status(Some(9),true)),1).unwrap();let read=f.backend.take_im_replies().pop().unwrap();assert_eq!(c.admit(&read).err(),Some("app_account_im_user_read_recipient_unconfirmed"));let rows:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(rows,0);assert!(f.scope.is_open());
}
#[test]
fn user_read_recipient_same_nonce_cannot_rebind_a_historical_uid_only_action(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();let alias=recipient_action(&f,id,&"a".repeat(32),uri());assert_eq!(alias.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_nonce_conflict"));assert_eq!(state(&f),(0,None,7));
}
#[test]
fn user_read_recipient_current_unknown_body_blocks_new_nonce_before_any_alias_query(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();let alias=recipient_action(&f,id,&"b".repeat(32),uri());assert_eq!(alias.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_uncertain"));assert_eq!(state(&f),(0,None,7));
}
#[test]
fn user_read_recipient_saved_intent_hash_is_data_and_different_alias_nonce_context_conflicts(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();p.cancel();f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_im_user_reads SET recipient_hash=?1",[hash_uri(uri())]).map_err(|_|"fixture")).unwrap();let same=recipient_action(&f,id,&"a".repeat(32),uri());let BoundUserRead::Replay(view)=same.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!("history only")};assert_eq!(view.state,"not_sent");let other=recipient_action(&f,id,&"a".repeat(32),b"[phone:+67890@fixture.invalid]");assert_eq!(other.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_nonce_conflict"));assert_eq!(state(&f),(3,None,7));
}
#[test]
fn user_read_recipient_debug_and_history_projection_never_export_uri_uid_or_hash(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=recipient_action(&f,id,&"a".repeat(32),uri());let BoundUserRead::Context(c)=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!()};let text=format!("{a:?} {c:?}");for private in ["12345","1_2","fixture","recipient_hash","token"]{assert!(!text.contains(private));}let original=action(&f,id,&"b".repeat(32));let _p=checked(&mut f,&original,Some(9),true).unwrap();let text=serde_json::to_string(&f.temp.database.with_conn(|c|n::im_inbox::user_read_history(c,f.run.line_id(),16)).unwrap()).unwrap();assert!(!text.contains("recipient_hash")&&!text.contains("12345"));
}
#[test]
fn user_read_recipient_retired_parent_or_foreign_native_binding_cannot_create_lookup_context(){
    let mut f=Fixture::new(true);let other=Fixture::new(true);let id=message_id(&mut f,1,7);let a=recipient_action(&f,id,&"a".repeat(32),uri());assert!(a.bind_current(other.backend.captured_im_binding().unwrap()).is_err());f.gate.retire();assert!(a.bind_current(f.backend.captured_im_binding().unwrap()).is_err());assert!(f.scope.is_open());
}
#[test]
fn user_read_recipient_schema_migration_adds_only_nullable_hash_and_preserves_historical_uid_row(){
    let conn=rusqlite::Connection::open_in_memory().unwrap();conn.execute_batch("CREATE TABLE app_voice_im_user_reads(id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),message_id INTEGER NOT NULL CHECK(message_id>0),target_index INTEGER NOT NULL CHECK(typeof(target_index)='integer' AND target_index>=0),fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),epoch_ms INTEGER NOT NULL CHECK(epoch_ms>=0),run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),request_id INTEGER CHECK(request_id>0),result_code INTEGER,updated_at TEXT NOT NULL,UNIQUE(line_id,user_nonce)); INSERT INTO app_voice_im_user_reads VALUES(1,'local-line',1,zeroblob(32),X'5032502f315f32',3,7,zeroblob(32),'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',1,'run','epoch',0,NULL,NULL,'time');").unwrap();let before:(i64,i64,i64,String)=conn.query_row("SELECT id,message_id,target_index,user_nonce FROM app_voice_im_user_reads",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();n::im_inbox::initialize_schema(&conn).unwrap();let after:(i64,i64,i64,String)=conn.query_row("SELECT id,message_id,target_index,user_nonce FROM app_voice_im_user_reads",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();assert_eq!(before,after);let hash:Option<Vec<u8>>=conn.query_row("SELECT recipient_hash FROM app_voice_im_user_reads",[],|r|r.get(0)).unwrap();assert!(hash.is_none());assert!(conn.execute("UPDATE app_voice_im_user_reads SET recipient_hash=X'01'",[]).is_err());n::im_inbox::initialize_schema(&conn).unwrap();
}
