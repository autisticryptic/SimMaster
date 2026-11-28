//! Actions-only original call-ticket liveness on a committed consumer claim.
use super::*;
use n::{native_invite_notifications::ConsumerClaim,native_invite_receipts::{self,InviteConsumptionReceipt},account_test_support::{TempDatabase,config},account_runtime::local_nonce};
use std::time::{Duration,Instant};
// Prepare slow synthetic RSA/schema/account/history inputs BEFORE a call ticket
// and its unchanged one-second ConsumerClaim exist. Receipt consumption itself
// remains the actual SQLite transaction at the original test's call site.
struct ReceiptSetup{temp:TempDatabase,run:n::account_runs::RunToken,history:n::account_call_history::HistoryTicket}
std::thread_local!{static RECEIPT_SETUP:std::cell::RefCell<Option<ReceiptSetup>>=const{std::cell::RefCell::new(None)};}
fn native_context()->(ArcRpcSession,AttemptOwner,RpcConnectionScope,CallDispatcher){
    RECEIPT_SETUP.with(|slot|{let needed=slot.borrow().is_none();if needed{
        let temp=TempDatabase::new();let cert=n::tls_engine::tests::Certificates::generate("DNS:fixture.invalid");let line=format!("app-account-{}",local_nonce().unwrap());
        temp.database.with_conn(|conn|n::account_store::create(conn,&line,&config(&cert.root),100)).unwrap();let run=temp.database.with_conn(|conn|n::account_runs::register_runtime(conn,&line,1,&local_nonce().unwrap())).unwrap();
        let history=n::account_call_history::insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();*slot.borrow_mut()=Some(ReceiptSetup{temp,run,history});
    }});
    super::native_context()
}
fn receipt(id:u64,fingerprint:[u8;32])->InviteConsumptionReceipt{
    let setup=RECEIPT_SETUP.with(|slot|slot.borrow_mut().take()).expect("fixture prepared before original claim");
    native_invite_receipts::consume(&setup.temp.database,&setup.run,&setup.history,id,&fingerprint).unwrap()
}
#[test]
fn committed_invite_claim_binds_original_live_call_and_cannot_survive_release(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()+Duration::from_secs(1));let proof=receipt(ID,fingerprint);
    assert!(claim.confirm(&proof,&ticket));assert!(claim.is_live());ticket.revoke();assert!(!claim.is_live());assert!(!claim.confirm(&proof,&ticket));
}
#[test]
fn historical_sqlite_receipt_after_call_revocation_cannot_confirm_original_invite_claim(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()+Duration::from_secs(1));
    ticket.revoke();let proof=receipt(ID,fingerprint);assert!(proof.matches(ID,&fingerprint));assert!(!claim.confirm(&proof,&ticket));assert!(scope.is_open());
}
#[test]
fn committed_invite_claim_rejects_wrong_call_bits_before_confirming_a_group(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID+1,fingerprint,Instant::now()+Duration::from_secs(1));
    assert!(!claim.confirm(&receipt(ID+1,fingerprint),&ticket));assert!(ticket.is_live());
}
#[test]
fn committed_invite_claim_rejects_foreign_call_owner_scope_despite_equal_id(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let original=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let(mut other,foreign,foreign_scope,foreign_dispatcher)=native_context();let ticket=native_ticket(&mut other,&foreign,&foreign_scope,&foreign_dispatcher,None);let fingerprint=[7;32];
    let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()+Duration::from_secs(1));let proof=receipt(ID,fingerprint);assert!(!claim.confirm(&proof,&ticket));assert!(claim.confirm(&proof,&original));assert!(original.is_live());assert!(ticket.is_live());
}
#[tokio::test]
async fn confirmed_invite_claim_cancel_wait_is_woken_by_original_call_revocation(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()+Duration::from_secs(1));assert!(claim.confirm(&receipt(ID,fingerprint),&ticket));
    let cancelled=claim.cancelled();tokio::pin!(cancelled);tokio::select!{_=cancelled.as_mut()=>panic!("original call still live"),_=tokio::time::sleep(Duration::from_millis(1))=>{}}ticket.revoke();tokio::time::timeout(Duration::from_secs(1),cancelled).await.unwrap();assert!(!claim.is_live());
}
#[tokio::test]
async fn unconfirmed_waiter_rechecks_new_original_call_binding_and_then_revocation(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()+Duration::from_secs(1));let cancelled=claim.cancelled();tokio::pin!(cancelled);
    tokio::select!{_=cancelled.as_mut()=>panic!("not cancelled"),_=tokio::time::sleep(Duration::from_millis(1))=>{}}assert!(claim.confirm(&receipt(ID,fingerprint),&ticket));ticket.revoke();tokio::time::timeout(Duration::from_secs(1),cancelled).await.unwrap();
}
#[test]
fn expired_unconsumed_sql_receipt_cannot_confirm_an_original_live_call_or_acquire_replay_permission(){
    let(mut rpc,owner,scope,dispatcher)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&dispatcher,None);let fingerprint=[7;32];let claim=ConsumerClaim::new(&owner,&scope,ID,fingerprint,Instant::now()-Duration::from_millis(1));let proof=receipt(ID,fingerprint);
    assert!(proof.matches(ID,&fingerprint));assert!(ticket.is_live());assert!(!claim.confirm(&proof,&ticket));assert!(claim.is_live());assert!(scope.is_open());
}
