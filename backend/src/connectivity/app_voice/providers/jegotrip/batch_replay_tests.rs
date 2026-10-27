use super::*;
fn incoming(rpc:&mut ArcRpcSession,scheduler:&mut BatchScheduler,owner:&AttemptOwner,now:u64,
    sequences:&[i32],ack_next:u32,ack_bits:u32)->BatchDelivery {
    let(id,_)=submit(rpc,scheduler,now);
    let requests=sequences.iter().map(|&sequence|(sequence,b"method".as_slice(),b"args".as_slice())).collect::<Vec<_>>();
    let bytes=response(id,BatchWords{next_batch:1,batches:1,ack_incarnation:7,ack_next_batch:ack_next,ack_batches:ack_bits,..peer()},&requests,&[]);
    let reply=batch_reply(rpc,owner,bytes.expose_to_transport());scheduler.on_reply(&reply,now).unwrap().unwrap()
}
#[test]
fn queued_or_unacknowledged_result_does_not_mint_batch_retirement() {
    let(mut rpc,mut scheduler,_,owner)=setup();let delivery=incoming(&mut rpc,&mut scheduler,&owner,0,&[5],0,0);
    let ticket=delivery.requests[0].ticket();assert!(delivery.retirement.is_none());
    scheduler.complete_server_request(ticket,0,&[0,1,0,0,0,0]).unwrap();
    assert!(incoming(&mut rpc,&mut scheduler,&owner,1,&[],0,0).retirement.is_none());
    let proof=incoming(&mut rpc,&mut scheduler,&owner,2,&[],1,1).retirement.unwrap();
    assert_eq!(proof.acknowledged(),&[5]);assert_eq!(proof.through(),0);assert!(proof.matches(&owner,&rpc.connection_scope()));
    assert!(incoming(&mut rpc,&mut scheduler,&owner,3,&[],1,1).retirement.is_none());
}
#[test]
fn partial_ack_retires_only_results_in_that_exact_original_epoch() {
    let(mut rpc,mut scheduler,_,owner)=setup();let first=incoming(&mut rpc,&mut scheduler,&owner,0,&[1],0,0);
    scheduler.complete_server_request(first.requests[0].ticket(),0,b"result1").unwrap();
    let second=incoming(&mut rpc,&mut scheduler,&owner,1,&[2],0,0);
    scheduler.complete_server_request(second.requests[0].ticket(),0,b"result2").unwrap();
    let proof=incoming(&mut rpc,&mut scheduler,&owner,2,&[],2,2).retirement.unwrap();assert_eq!(proof.acknowledged(),&[1]);
    assert_eq!(scheduler.history_count(),1);
    let proof=incoming(&mut rpc,&mut scheduler,&owner,3,&[],2,1).retirement.unwrap();assert_eq!(proof.acknowledged(),&[2]);assert_eq!(scheduler.history_count(),0);
}
#[test]
fn lower_pending_and_unacked_result_hold_prefix_below_original_sequence() {
    let(mut rpc,mut scheduler,_,owner)=setup();let first=incoming(&mut rpc,&mut scheduler,&owner,0,&[1],0,0);
    let high=incoming(&mut rpc,&mut scheduler,&owner,1,&[8192],0,0);assert!(high.retirement.is_none());
    scheduler.complete_server_request(high.requests[0].ticket(),0,b"high").unwrap();
    let proof=incoming(&mut rpc,&mut scheduler,&owner,2,&[],1,1).retirement.unwrap();assert_eq!(proof.acknowledged(),&[8192]);assert_eq!(proof.through(),0);
    scheduler.complete_server_request(first.requests[0].ticket(),0,b"late low").unwrap();
    assert!(incoming(&mut rpc,&mut scheduler,&owner,3,&[],0,0).retirement.is_none(),"queued low result still blocks prefix");
    let proof=incoming(&mut rpc,&mut scheduler,&owner,4,&[],2,1).retirement.unwrap();assert_eq!(proof.acknowledged(),&[1]);assert_eq!(proof.through(),6175);
}
#[test]
fn before_base_retirement_does_not_reject_unseen_reordered_gap_inside_window() {
    let(mut rpc,mut scheduler,_,owner)=setup();let high=incoming(&mut rpc,&mut scheduler,&owner,0,&[3000],0,0);
    assert_eq!(high.retirement.unwrap().through(),959);
    let gap=incoming(&mut rpc,&mut scheduler,&owner,1,&[500,1000,3000],0,0);
    assert_eq!(gap.requests.iter().map(|r|r.ticket().sequence()).collect::<Vec<_>>(),vec![1000]);
    assert!(gap.retirement.is_none());assert!(scheduler.is_open());
}
#[test]
fn foreign_reply_and_wrong_ack_cannot_mint_original_retirement() {
    let(mut rpc,mut scheduler,_,owner)=setup();let(mut other_rpc,mut other,_,other_owner)=setup();
    let delivery=incoming(&mut rpc,&mut scheduler,&owner,0,&[1],0,0);
    scheduler.complete_server_request(delivery.requests[0].ticket(),0,b"original").unwrap();
    let(id,_)=submit(&mut other_rpc,&mut other,1);let bytes=response(id,peer(),&[],&[]);let reply=batch_reply(&mut other_rpc,&other_owner,bytes.expose_to_transport());
    assert!(scheduler.on_reply(&reply,1).unwrap().is_none());assert_eq!(scheduler.working.results.len(),1);
    let(id,_)=submit(&mut rpc,&mut scheduler,2);let bytes=response(id,BatchWords{ack_incarnation:8,ack_next_batch:1,ack_batches:1,..peer()},&[],&[]);
    let reply=batch_reply(&mut rpc,&owner,bytes.expose_to_transport());assert_eq!(scheduler.on_reply(&reply,2).err(),Some(BatchError::Epoch));assert!(!scheduler.is_open());
}
