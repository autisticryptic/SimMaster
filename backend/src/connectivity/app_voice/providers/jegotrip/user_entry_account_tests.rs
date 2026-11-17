//! Actions-only actual AccountManager/current run/SQL query isolation on the
//! same original bootstrap, native SDK and auxiliary physical peer fixture.
use super::*;
use super::super::super::super as n;
pub(super) async fn check(root:&str,port:u16,submitted:oneshot::Receiver<()>,cancel:bool){
    let temp=n::account_test_support::TempDatabase::new();let manager=n::account_runtime::AccountManager::new(temp.database.clone());let created=manager.create(bootstrap_account_config(root,port),100).await.unwrap();let line=created.account.line_id.clone();manager.start(&line,1,100).await.unwrap();
    let before=tokio::time::timeout(Duration::from_secs(10),async{loop{let v=manager.get(&line,100).await.unwrap();if v.phase=="hosted"{break v;}tokio::task::yield_now().await;}}).await.unwrap();assert_eq!(before.user_entry_supported,Some(true));
    let recipient="[phone:+12345@account-domain.invalid]";
    assert_eq!(manager.lookup_user_entry(&line,1,recipient,false).await.err(),Some("app_account_user_entry_consent_required"));assert_eq!(manager.lookup_user_entry(&line,2,recipient,true).await.err(),Some("app_account_revision_conflict"));assert_eq!(manager.lookup_user_entry(&line,1,"[phone:+12345@foreign.invalid]",true).await.err(),Some("app_account_user_entry_realm_mismatch"));
    let counts=||temp.database.with_conn(|c|{let mut values=Vec::new();for table in ["sms_messages","app_voice_im_messages","app_voice_im_user_reads","app_voice_im_read_receipts"]{values.push(c.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).map_err(|_|"fixture-count")?);}Ok(values)}).unwrap();let before_counts=counts();
    let mut lookup=Box::pin(manager.lookup_user_entry(&line,1,recipient,true));if cancel{tokio::select!{_=submitted=>(),result=&mut lookup=>panic!("current lookup returned before cancellation: {result:?}")};drop(lookup);}else{let observed=lookup.await.unwrap();assert_eq!(observed.report,"reported");assert_eq!(observed.kind,"phone");let json=serde_json::to_string(&observed).unwrap();for secret in ["12345","7_9","fixture","token","UserEntry","uid"]{assert!(!json.contains(secret));}}
    let after=manager.get(&line,100).await.unwrap();assert_eq!(after.phase,"hosted");assert_eq!(before.incoming_message_ready,after.incoming_message_ready);assert_eq!(before.incoming_call_ready,after.incoming_call_ready);assert_eq!(before.outbound_authorized,after.outbound_authorized);assert_eq!(before_counts,counts());
    let stopped=manager.stop(&line,1).await.unwrap();assert!(!stopped.io_cleanup_required);assert!(!stopped.incoming_message_ready&&!stopped.incoming_call_ready&&!stopped.outbound_authorized);manager.shutdown().await;
}
#[tokio::test]async fn user_entry_account_current_revision_consent_and_realm_read_have_no_message_receipt_or_ready_mutation(){Box::pin(super::scenario(10)).await;}
#[tokio::test]async fn user_entry_account_cancelled_http_wait_preserves_hosted_sdk_then_stops_with_all_original_joins(){Box::pin(super::scenario(11)).await;}
