//! Actions-only startup future descriptor regression. No execution of start,
//! socket, login, new stack, timeout change or other-device action occurs here.
use super::*;
#[test]
fn original_account_start_outer_future_stays_small_for_standard_join_select_callers(){
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());
    let future=manager.start("app-account-0123456789abcdef0123456789abcdef",1,100);
    let bytes=std::mem::size_of_val(&future);
    assert!(bytes<=4096,"heap-owned original start descriptor must not inline the account/runtime startup state: {bytes}");
    drop(future);
    assert_eq!(temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases",[],|row|row.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap(),0);
}
