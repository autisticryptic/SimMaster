//! Actions-only run fence/SQLite lifecycle, not proof of actual actor Join.
//! The companion Common account cases perform the real controlled Join path.
use super::*;
fn sql_count(db:&crate::platform::db::Database,table:&str)->i64{db.with_conn(|c|c.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
fn fixture()->Fixture{
    let mut f=Fixture::new(true);let notice=f.hint(1,99,b"P2P/1_2",7);let scope=f.inbox(&notice);let first=pending(&scope,&f.temp.database,&notice);let message=f.message(2,b"P2P/1_2",7,r#"{"text":"kept history"}"#);let proof=scope.commit(&f.temp.database,&message).unwrap().unwrap();scope.finish_association(&f.temp.database,&first,Some(&proof)).unwrap();f
}
#[test]
fn im_association_closed_scope_is_not_a_release_or_permission_to_sweep_original_ledger(){
    let f=fixture();f.scope.close();assert_eq!(rows(&f),1);assert!(f.temp.database.with_conn(|c|account_runs::runtime_held(c,f.run.line_id())).unwrap());assert_eq!(f.count(),1);
}
#[test]
fn im_association_quiescence_fence_compacts_association_without_removing_message_or_event_history(){
    let f=fixture();let line=f.run.line_id().to_owned();f.scope.close();let Fixture{temp,run,..}=f;let released=n::account_runtime::test_quiescence(Arc::try_unwrap(run).unwrap());temp.database.with_conn(|c|account_runs::release_and_compact(c,&released)).unwrap();
    assert_eq!(sql_count(&temp.database,"app_voice_im_notifications"),0);assert_eq!(sql_count(&temp.database,"app_voice_im_messages"),1);assert_eq!(temp.database.get_recent_app_events(Some(&line),16).unwrap().len(),1);assert!(!temp.database.with_conn(|c|account_runs::ownership_pending(c,&line)).unwrap());temp.database.with_conn(|c|account_runs::release_and_compact(c,&released)).unwrap();
}
#[test]
fn im_association_compaction_error_rolls_back_original_run_release_until_same_proof_completes(){
    let f=fixture();let line=f.run.line_id().to_owned();f.scope.close();let Fixture{temp,run,..}=f;let released=n::account_runtime::test_quiescence(Arc::try_unwrap(run).unwrap());
    temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fail_im_gc BEFORE DELETE ON app_voice_im_notifications BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();assert!(temp.database.with_conn(|c|account_runs::release_and_compact(c,&released)).is_err());assert_eq!(sql_count(&temp.database,"app_voice_im_notifications"),1);assert!(temp.database.with_conn(|c|account_runs::ownership_pending(c,&line)).unwrap());
    temp.database.with_conn(|c|c.execute_batch("DROP TRIGGER fail_im_gc").map_err(|_|"fixture")).unwrap();temp.database.with_conn(|c|account_runs::release_and_compact(c,&released)).unwrap();assert_eq!(sql_count(&temp.database,"app_voice_im_notifications"),0);assert!(!temp.database.with_conn(|c|account_runs::ownership_pending(c,&line)).unwrap());assert_eq!(sql_count(&temp.database,"app_voice_im_messages"),1);
}
#[test]
fn im_association_explicit_account_delete_is_fenced_and_removes_only_its_private_im_history(){
    let f=fixture();let line=f.run.line_id().to_owned();assert!(f.temp.database.with_conn(|c|n::im_inbox::delete_account(c,&line)).is_err());assert!(f.temp.database.with_conn(|c|account_store::delete(c,&line,1)).is_err());assert_eq!(f.count(),1);
    let other="app-account-fedcba9876543210fedcba9876543210";let cfg=f.temp.database.with_conn(|c|account_store::load(c,&line)).unwrap().unwrap().config;f.temp.database.with_conn(|c|account_store::create(c,other,&cfg,100)).unwrap();
    f.temp.database.with_conn(|c|c.execute("INSERT INTO app_voice_im_messages(line_id,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at) SELECT ?1,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at FROM app_voice_im_messages WHERE line_id=?2",rusqlite::params![other,line]).map_err(|_|"fixture")).unwrap();
    f.scope.close();let Fixture{temp,run,..}=f;let released=n::account_runtime::test_quiescence(Arc::try_unwrap(run).unwrap());temp.database.with_conn(|c|account_runs::release_and_compact(c,&released)).unwrap();assert!(temp.database.with_conn(|c|account_store::delete(c,&line,1)).unwrap());assert_eq!(sql_count(&temp.database,"app_voice_im_messages"),1);assert_eq!(temp.database.with_conn(|c|n::im_inbox::list(c,other,16)).unwrap()[0].text,"kept history");assert!(temp.database.with_conn(|c|account_store::metadata(c,&line)).unwrap().is_none());
}
