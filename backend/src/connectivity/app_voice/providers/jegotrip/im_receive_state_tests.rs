//! Actions-only proposed ordering DATA model tests; no cursor/IO grant.
use crate::connectivity::app_voice::providers::jegotrip::{im_receive_state::{ImReceiveState,Gap},im_data::ImBox,im_status,rpc_stream::{Writer,RpcProperty}};
fn status(rows:&[(&[u8],i64,i64)],time:i64,accepted:bool)->Vec<u8>{
    let mut w=Writer::new();w.boolean(accepted).unwrap();w.u32(rows.len()as u32).unwrap();for(label,base,target)in rows{w.bytes(label).unwrap();let base=base.to_string();let target=target.to_string();w.properties(&[RpcProperty{key:im_status::PEER_READ,value:base.as_bytes()},RpcProperty{key:im_status::LATEST,value:target.as_bytes()}]).unwrap();}w.i64(time).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn apply(s:&mut ImReceiveState,rows:&[(&[u8],i64,i64)],time:i64){assert!(s.observe_status(&im_status::decode_refresh(0,&status(rows,time,true)).unwrap()).unwrap());}
#[test]
fn im_ordering_starts_with_timestamp_negative_one_but_no_guessed_box_or_receive_cursor(){
    let mut state=ImReceiveState::new();assert_eq!(state.since(),-1);assert!(!state.has_initial_status());assert!(state.next_gap().unwrap().is_none());assert!(state.observed_target(ImBox::parse(b"P2P/1_2").unwrap(),7).is_err());assert!(state.next_gap().unwrap().is_none());
}
#[test]
fn im_ordering_three_original_gap_batches_keep_ten_limit_and_only_committed_positions_advance(){
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",0,25)],1);let box_id=ImBox::parse(b"P2P/1_2").unwrap();
    for (start,count)in[(1,10),(11,10),(21,5)]{let(key,gap)=state.next_gap().unwrap().unwrap();assert_eq!(key.as_slice(),box_id.bytes());assert_eq!(gap,Gap{start,count});assert_eq!(state.consumed(box_id),Some((0,start-1)));state.committed_gap(box_id,gap).unwrap();}
    assert!(state.next_gap().unwrap().is_none());assert_eq!(state.consumed(box_id),Some((0,25)));
}
#[test]
fn im_ordering_missing_reported_peer_read_does_not_make_initial_refresh_complete_or_seed_zero(){
    let mut w=Writer::new();w.boolean(true).unwrap();w.u32(1).unwrap();w.bytes(b"1_2").unwrap();w.properties(&[RpcProperty{key:im_status::LATEST,value:b"7"}]).unwrap();w.i64(1).unwrap();w.bytes(b"").unwrap();let bytes=w.finish();let mut state=ImReceiveState::new();state.observe_status(&im_status::decode_refresh(0,bytes.expose_to_transport()).unwrap()).unwrap();assert!(!state.has_initial_status());assert!(state.next_gap().unwrap().is_none());assert_eq!(state.consumed(ImBox::parse(b"P2P/1_2").unwrap()),None);
}
#[test]
fn im_ordering_failed_or_old_refresh_cannot_change_timestamp_or_existing_gap(){
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",3,7)],10);let before=state.next_gap().unwrap().unwrap().1;
    assert!(state.observe_status(&im_status::decode_refresh(0,&status(&[(b"1_2",7,99)],11,false)).unwrap()).is_err());assert_eq!(state.since(),10);
    assert!(!state.observe_status(&im_status::decode_refresh(0,&status(&[(b"1_2",7,99)],9,true)).unwrap()).unwrap());assert_eq!(state.next_gap().unwrap().unwrap().1,before);
}
#[test]
fn im_ordering_new_remote_peer_read_is_not_local_consumption_or_permission_to_skip_a_gap(){
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",3,7)],10);apply(&mut state,&[(b"1_2",999,1000)],11);let box_id=ImBox::parse(b"P2P/1_2").unwrap();assert_eq!(state.consumed(box_id),Some((3,3)));assert_eq!(state.next_gap().unwrap().unwrap().1,Gap{start:4,count:10});
}
#[test]
fn im_ordering_target_growth_does_not_change_an_already_requested_original_small_span(){
    let mut state=ImReceiveState::new();let box_id=ImBox::parse(b"P2P/1_2").unwrap();apply(&mut state,&[(b"1_2",0,2)],1);let original=state.next_gap().unwrap().unwrap().1;assert_eq!(original,Gap{start:1,count:2});state.observed_target(box_id,20).unwrap();state.committed_gap(box_id,original).unwrap();assert_eq!(state.next_gap().unwrap().unwrap().1,Gap{start:3,count:10});
}
#[test]
fn im_ordering_invalid_or_noncontiguous_commit_preserves_original_cursor(){
    let mut state=ImReceiveState::new();let box_id=ImBox::parse(b"P2P/1_2").unwrap();apply(&mut state,&[(b"1_2",0,20)],1);
    for gap in [Gap{start:2,count:1},Gap{start:1,count:0},Gap{start:1,count:11},Gap{start:i64::MAX,count:2}]{assert!(state.committed_gap(box_id,gap).is_err());assert_eq!(state.consumed(box_id),Some((0,0)));}
}
#[test]
fn im_ordering_signed_maximum_target_and_explicit_negative_one_base_do_not_overflow(){
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",-1,i64::MAX)],1);assert_eq!(state.next_gap().unwrap().unwrap().1,Gap{start:0,count:10});
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",i64::MAX-1,i64::MAX)],1);let box_id=ImBox::parse(b"P2P/1_2").unwrap();let gap=state.next_gap().unwrap().unwrap().1;assert_eq!(gap,Gap{start:i64::MAX,count:1});state.committed_gap(box_id,gap).unwrap();assert!(state.next_gap().unwrap().is_none());
}
#[test]
fn im_ordering_new_box_after_confirmed_initial_snapshot_uses_actual_first_message_not_notification(){
    let mut state=ImReceiveState::new();apply(&mut state,&[],1);assert!(state.has_initial_status());let box_id=ImBox::parse(b"Org/1-2").unwrap();state.observed_target(box_id,7).unwrap();assert_eq!(state.consumed(box_id),Some((6,6)));assert_eq!(state.next_gap().unwrap().unwrap().1,Gap{start:7,count:1});assert!(state.observed_target(box_id,-1).is_err());
}
#[test]
fn im_ordering_box_capacity_rejects_a_whole_new_snapshot_without_partial_timestamp_or_state(){
    let names=(1..=128).map(|n|format!("{n}_1")).collect::<Vec<_>>();let rows=names.iter().map(|name|(name.as_bytes(),0,0)).collect::<Vec<_>>();let mut state=ImReceiveState::new();apply(&mut state,&rows,1);
    let data=status(&[(b"999_1",0,1)],2,true);assert!(state.observe_status(&im_status::decode_refresh(0,&data).unwrap()).is_err());assert_eq!(state.since(),1);assert!(state.next_gap().unwrap().is_none());
}
#[test]
fn im_ordering_keys_and_state_debug_remain_private_and_do_not_serialize_source_identity(){
    let mut state=ImReceiveState::new();apply(&mut state,&[(b"1_2",0,1)],1);assert_eq!(format!("{state:?}"),"ImReceiveState(<redacted>)");assert!(!format!("{state:?}").contains("1_2"));
}
