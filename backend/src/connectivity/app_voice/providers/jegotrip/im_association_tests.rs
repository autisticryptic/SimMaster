//! Actions-only durable association tests. No SDK process, device or live IO.
use super::*;
use n::im_inbox::{AssociationAdmission,ImPendingAssociation};
#[path="im_association_lifecycle_tests.rs"]
mod lifecycle;
fn pending(scope:&ImInboxScope,db:&crate::platform::db::Database,delivery:&IncomingImDelivery)->ImPendingAssociation{
    match scope.open_association(db,delivery).unwrap(){AssociationAdmission::Fresh(pending)=>pending,_=>panic!("only first original pending capability")}
}
fn replay(scope:&ImInboxScope,db:&crate::platform::db::Database,delivery:&IncomingImDelivery)->Option<n::im_inbox::CommittedIm>{
    match scope.open_association(db,delivery).unwrap(){AssociationAdmission::Replay(proof)=>proof,_=>panic!("existing exact association must not acquire a fresh deadline")}
}
fn rows(f:&Fixture)->i64{f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_notifications",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
fn outcome(f:&Fixture)->i64{f.temp.database.with_conn(|c|c.query_row("SELECT outcome FROM app_voice_im_notifications LIMIT 1",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
#[test]
fn im_association_native_message_and_clientex_share_one_original_receipt_epoch(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"original"}"#);let notice=f.hint(2,99,b"P2P/1_2",7);
    assert!(Arc::ptr_eq(message.notice().binding(),notice.notice().binding()));assert_eq!(message.notice().binding().receipt_epoch(),notice.notice().binding().receipt_epoch());
    let second=batch(&mut f,NOTIFY_CALLS_METHOD.as_bytes(),&hint(100,b"P2P/1_2",7));assert!(Arc::ptr_eq(message.notice().binding(),second.notice().binding()));assert_eq!(message.notice().binding().receipt_epoch().len(),32);
}
#[test]
fn im_association_first_pending_is_not_consumed_and_only_committed_matching_message_completes_it(){
    let mut f=Fixture::new(true);let notice=f.hint(1,0xf123456789abcdef,b"P2P/1_2",7);let context=f.inbox(&notice);let first=pending(&context,&f.temp.database,&notice);assert_eq!(outcome(&f),0);assert_eq!(f.count(),0);
    let message=f.message(2,b"P2P/1_2",7,r#"{"text":"durable"}"#);let proof=f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();let committed=context.finish_association(&f.temp.database,&first,Some(&proof)).unwrap().unwrap();assert!(committed.authorizes(notice.ticket(),Instant::now()));assert_eq!(outcome(&f),2);assert_eq!(rows(&f),1);
    assert_eq!(result(f.backend.complete_im(notice.ticket(),Some(&committed)).unwrap()),[0,1,0,0,0,0]);assert_eq!(f.count(),1);
}
#[test]
fn im_association_terminal_consumed_replay_survives_queue_eviction_without_repeating_text_event(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"Org/1-2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"Org/1-2",7,r#"{"text":"once"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();scope.finish_association(&f.temp.database,&first,Some(&proof)).unwrap();drop(first);drop(scope);
    let again=f.hint(3,99,b"Org/1-2",7);let proof=replay(&f.inbox(&again),&f.temp.database,&again).unwrap();assert!(proof.authorizes(again.ticket(),Instant::now()));assert_eq!(rows(&f),1);assert_eq!(f.count(),1);assert_eq!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap().len(),1);
}
#[test]
fn im_association_changed_box_or_index_cannot_rebind_an_evicted_consumed_notification_id(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);scope.finish_association(&f.temp.database,&first,None).unwrap();
    for(rpc,box_id,index)in[(2,b"Org/1-2".as_slice(),7),(3,b"P2P/1_2".as_slice(),8)]{let changed=f.hint(rpc,99,box_id,index);assert_eq!(f.inbox(&changed).open_association(&f.temp.database,&changed).err(),Some("app_account_im_notification_conflict"));}assert_eq!(rows(&f),1);assert_eq!(outcome(&f),1);
}
#[test]
fn im_association_negative_tombstone_cannot_be_revived_by_later_durable_data(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);assert!(scope.finish_association(&f.temp.database,&first,None).unwrap().is_none());
    let later=f.message(2,b"P2P/1_2",7,r#"{"text":"late"}"#);scope.commit(&f.temp.database,&later).unwrap();let again=f.hint(3,99,b"P2P/1_2",7);assert!(replay(&scope,&f.temp.database,&again).is_none());assert_eq!(outcome(&f),1);assert_eq!(f.count(),1);
}
#[test]
fn im_association_abandoned_pending_row_does_not_manufacture_a_new_first_deadline(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);drop(first);let again=f.hint(2,99,b"P2P/1_2",7);assert!(again.ticket().deadline()>=notice.ticket().deadline());assert!(replay(&scope,&f.temp.database,&again).is_none());assert_eq!(outcome(&f),1);
}
#[test]
fn im_association_deleted_text_keeps_exact_private_consumption_but_never_resurrects_body(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"P2P/1_2",7,r#"{"text":"deleted"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();scope.finish_association(&f.temp.database,&first,Some(&proof)).unwrap();
    let id=f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap()[0].id;f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),id)).unwrap();let again=f.hint(3,99,b"P2P/1_2",7);assert!(replay(&scope,&f.temp.database,&again).unwrap().authorizes(again.ticket(),Instant::now()));assert!(f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().is_empty());assert_eq!(rows(&f),1);assert_eq!(f.count(),1);
}
#[test]
fn im_association_same_row_id_with_changed_message_fingerprint_cannot_redeem_consumption(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"P2P/1_2",7,r#"{"text":"original"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();scope.finish_association(&f.temp.database,&first,Some(&proof)).unwrap();
    f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_im_messages SET fingerprint=?1",[vec![0u8;32]]).map_err(|_|"fixture")).unwrap();let again=f.hint(3,99,b"P2P/1_2",7);assert!(replay(&scope,&f.temp.database,&again).is_none());assert_eq!(rows(&f),1);
}
#[test]
fn im_association_scope_revoke_or_parent_retirement_does_not_finish_pending_positive(){
    for revoke in [false,true]{let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"P2P/1_2",7,r#"{"text":"stored"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();if revoke{f.scope.mark_session_revoked();}else{f.gate.retire();}assert!(scope.finish_association(&f.temp.database,&first,Some(&proof)).is_err());assert_eq!(outcome(&f),0);assert_eq!(f.count(),1);}
}
#[test]
fn im_association_foreign_current_scope_or_revision_never_reuses_pending_capability(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let mut other=Fixture::new(true);let foreign=other.hint(1,99,b"P2P/1_2",7);assert!(other.inbox(&foreign).finish_association(&other.temp.database,&first,None).is_err());
    f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET revision=2",[]).map_err(|_|"fixture")).unwrap();assert!(scope.finish_association(&f.temp.database,&first,None).is_err());assert_eq!(outcome(&f),0);
}
#[test]
fn im_association_update_failure_rolls_back_consumption_marker_but_keeps_valid_local_message(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"P2P/1_2",7,r#"{"text":"already stored"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fail_im_association BEFORE UPDATE ON app_voice_im_notifications WHEN NEW.outcome=2 BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();assert!(scope.finish_association(&f.temp.database,&first,Some(&proof)).is_err());assert_eq!(outcome(&f),0);assert_eq!(f.count(),1);
}
#[test]
fn im_association_first_capability_finishes_once_and_cannot_overwrite_terminal_outcome(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);scope.finish_association(&f.temp.database,&first,None).unwrap();assert!(scope.finish_association(&f.temp.database,&first,None).is_err());assert_eq!(rows(&f),1);assert_eq!(outcome(&f),1);
}
#[test]
fn im_association_more_than_sixty_four_completed_ids_are_durable_without_expanding_live_queue_budget(){
    let mut f=Fixture::new(true);let message=f.message(1,b"P2P/1_2",7,r#"{"text":"same data"}"#);let scope=f.inbox(&message);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();f.backend.complete_im(message.ticket(),Some(&proof)).unwrap();
    for rpc in 2..=81{let notice=f.hint(rpc,rpc as u64,b"P2P/1_2",7);let first=pending(&scope,&f.temp.database,&notice);let consumed=scope.finish_association(&f.temp.database,&first,Some(&proof)).unwrap().unwrap();assert_eq!(result(f.backend.complete_im(notice.ticket(),Some(&consumed)).unwrap()),[0,1,0,0,0,0]);}assert_eq!(rows(&f),80);assert_eq!(f.count(),1);
}
#[test]
fn im_association_per_account_capacity_preserves_all_existing_terminal_and_pending_ids(){
    let mut f=Fixture::new(true);for rpc in 1..=1024{let notice=f.hint(rpc,rpc as u64,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);scope.finish_association(&f.temp.database,&first,None).unwrap();f.backend.complete_im(notice.ticket(),None).unwrap();}
    let excess=f.hint(1025,1025,b"P2P/1_2",7);assert_eq!(f.inbox(&excess).open_association(&f.temp.database,&excess).err(),Some("app_account_im_association_full"));assert_eq!(rows(&f),1024);let old=f.hint(1026,1,b"P2P/1_2",7);assert!(replay(&f.inbox(&old),&f.temp.database,&old).is_none());assert_eq!(rows(&f),1024);
}
#[test]
fn im_association_global_capacity_cannot_sweep_other_accounts_to_admit_new_work(){
    let mut f=Fixture::new(true);let other="app-account-fedcba9876543210fedcba9876543210";let cfg=f.temp.database.with_conn(|c|account_store::load(c,f.run.line_id())).unwrap().unwrap().config;f.temp.database.with_conn(|c|account_store::create(c,other,&cfg,100)).unwrap();
    f.temp.database.with_conn(|c|c.execute("WITH RECURSIVE ids(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<4096) INSERT INTO app_voice_im_notifications(line_id,revision,run_nonce,scope_epoch,notification_id,identity_key,key_fingerprint,notice_fingerprint,outcome) SELECT ?1,1,?2,?3,CAST(printf('%08d',n) AS BLOB),?4,?4,?4,1 FROM ids",rusqlite::params![other,"a".repeat(32),"b".repeat(32),vec![0u8;32]]).map_err(|_|"fixture")).unwrap();
    let notice=f.hint(1,99,b"P2P/1_2",7);assert_eq!(f.inbox(&notice).open_association(&f.temp.database,&notice).err(),Some("app_account_im_association_full"));assert_eq!(rows(&f),4096);
}
#[test]
fn im_association_private_debug_never_exports_ids_fingerprints_or_scope_epoch(){
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let text=format!("{first:?} {scope:?} {:?}",notice.notice().binding());for private in ["P2P/1_2","fixture-auth",notice.notice().binding().receipt_epoch()]{assert!(!text.contains(private));}
}
