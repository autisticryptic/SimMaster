//! Pure original sender-snapshot accounting; no IO, grant or result replay.
use super::*;
#[test]
fn im_sender_identity_snapshot_reserves_before_copy_or_oneshot_and_full_budget_cannot_consume_id(){
    let(mut rpc,who,scope)=current_native();let id=rpc.next_id;let pending=rpc.pending.len();
    let full=scope.reserve_native_rpc(131072).unwrap();assert!(rpc.send_identity().is_err());
    assert_eq!(scope.native_rpc_resources(),(1,131072));assert_eq!(rpc.next_id,id);assert_eq!(rpc.pending.len(),pending);
    assert!(scope.is_open());drop(full);
    let identity=rpc.send_identity().unwrap();assert!(identity.matches(&who,&scope));
    assert_eq!(scope.native_rpc_resources(),(1,identity.retained_bytes()));
    drop(identity);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn im_sender_identity_snapshot_remains_charged_until_last_arc_after_original_close_and_revocation(){
    let(mut rpc,who,scope)=current_native();let identity=rpc.send_identity().unwrap();let held=identity.clone();
    let cost=scope.native_rpc_resources();assert_eq!(cost.0,1);assert!(cost.1>0);
    rpc.close();scope.mark_session_revoked();assert!(!held.matches(&who,&scope));
    drop(identity);assert_eq!(scope.native_rpc_resources(),cost,"original close/actor take cannot refund retained secret DATA");
    drop(held);assert_eq!(scope.native_rpc_resources(),(0,0));
}
