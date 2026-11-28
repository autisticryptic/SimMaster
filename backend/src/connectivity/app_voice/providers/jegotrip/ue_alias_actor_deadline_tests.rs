//! Actual Common deadline regression; the original shorter-deadline test is unchanged.
use super::*;
use super::ue_alias::{Fixture,Mode,bounded};

#[tokio::test]
async fn ue_alias_result_deadline_retires_without_closing_common_before_late_reply(){Box::pin(async{
    let mut f=Box::pin(Fixture::open(Mode::PolicyTimeout)).await;let command=f.handle.command_handle();let action=f.action(3000);
    let mut reading=Box::pin(command.read_aliases(action.clone(),&[]));
    tokio::select!{r=reading.as_mut()=>panic!("reply still held: {r:?}"),r=bounded(&mut f.seen)=>r.unwrap()}
    assert_eq!(bounded(reading).await.err(),Some(RuntimeError::Timeout));
    assert!(f.scope.is_open(),"a fully submitted READ timeout does not retire the original SDK");
    assert!(Instant::now()<action.deadline());
    bounded(command.original_user_entry_auth(&f.owner,&f.scope)).await.unwrap();
    f.release.take().unwrap().send(()).unwrap();bounded(command.poll(&f.owner,&f.scope,&[])).await.unwrap();
    let scope=f.scope.clone();f.close(true).await;drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
}).await;}
