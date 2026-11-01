use super::*;

fn core(now:Instant)->ChannelCore{ChannelCore::new(0,1460,now+Duration::from_secs(10),Duration::from_secs(2),now).unwrap()}
fn wire(sequence:u16,kind:PacketKind,gap:u8,ack:u16,delivered:u16,mask:&[u8],inclusive:bool,body:&[u8])->TransportBytes{
 native_channel::encode_packet(HeaderMode::Basic,&ChannelPacket{timestamp:None,sequence,kind,data_gap:gap,
  ack_sequence:ack,delivered_sequence:delivered,inclusive_ack:inclusive,ack_mask:mask,body}).unwrap()
}
fn ready(now:Instant,peer:u16)->ChannelCore{
 let mut c=core(now);assert_eq!(c.next_packet(now).unwrap().unwrap().expose_to_transport(),[0x40,0,0,0,0,0,0,0]);
 let start=wire(peer,PacketKind::Start,0,0x4000,0x4000,&[],false,&[]);
 assert!(c.on_packet(start.expose_to_transport(),now).unwrap().is_empty());assert!(c.ready());
 let ack=c.next_packet(now).unwrap().unwrap();let ack=native_channel::decode_packet(HeaderMode::Basic,ack.expose_to_transport()).unwrap();
 assert_eq!(ack.kind,PacketKind::Probe);assert_eq!(ack.body,PROBE_MAGIC);assert_eq!(ack.delivered_sequence,peer);c
}
fn data(sequence:u16,gap:u8,body:&[u8])->TransportBytes{wire(sequence,PacketKind::Data,gap,0,0,&[],false,body)}
fn probe(sequence:u16,ack:u16,delivered:u16,mask:&[u8],inclusive:bool)->TransportBytes{wire(sequence,PacketKind::Probe,0,ack,delivered,mask,inclusive,&PROBE_MAGIC)}
fn send(c:&mut ChannelCore,body:&[u8],now:Instant)->TransportBytes{
 c.queue_record(body,now+Duration::from_secs(5),now).unwrap();c.next_packet(now).unwrap().unwrap()
}

#[test]
fn native_start_vector_uses_fresh_incarnation_not_rpc_sequence_one(){
 let now=Instant::now();for seed in [0,1,0x3fff,0xffff]{
  let mut c=ChannelCore::new(seed,1460,now+Duration::from_secs(1),Duration::from_secs(1),now).unwrap();
  let out=c.next_packet(now).unwrap().unwrap();let p=native_channel::decode_packet(HeaderMode::Basic,out.expose_to_transport()).unwrap();
  assert_eq!(p.sequence,(seed&0x3fff)+0x4000);assert_eq!(p.kind,PacketKind::Start);assert_eq!(p.data_gap,0);
  assert_eq!(p.ack_sequence,0);assert!(p.ack_mask.is_empty());assert!(p.body.is_empty());assert!(!c.ready());
 }
}

#[test]
fn received_start_and_local_start_ack_are_both_required_before_record_delivery(){
 let now=Instant::now();let mut c=core(now);c.next_packet(now).unwrap();
 let start=wire(0x9000,PacketKind::Start,0,0,0,&[],false,&[0xff,b'x']);
 assert!(c.on_packet(start.expose_to_transport(),now).unwrap().is_empty());assert!(!c.ready());assert_eq!(c.received.len(),1);
 let reply=probe(0x9001,0x4000,0x4000,&[],false);let out=c.on_packet(reply.expose_to_transport(),now).unwrap();
 assert!(c.ready());assert_eq!(out.len(),1);assert_eq!(out[0].expose_to_transport(),b"x");
}

#[test]
fn two_independent_channel_starts_complete_without_an_ack_probe_loop(){
 let now=Instant::now();let mut a=core(now);let mut b=ChannelCore::new(12,1460,now+Duration::from_secs(10),Duration::from_secs(2),now).unwrap();
 let sa=a.next_packet(now).unwrap().unwrap();let sb=b.next_packet(now).unwrap().unwrap();
 a.on_packet(sb.expose_to_transport(),now).unwrap();b.on_packet(sa.expose_to_transport(),now).unwrap();
 let mut sent=0;for _ in 0..12{
  let mut any=false;
  if let Some(p)=a.next_packet(now).unwrap(){b.on_packet(p.expose_to_transport(),now).unwrap();sent+=1;any=true;}
  if let Some(p)=b.next_packet(now).unwrap(){a.on_packet(p.expose_to_transport(),now).unwrap();sent+=1;any=true;}
  if !any{break;}
 }
 assert!(a.ready()&&b.ready());assert!(sent<=4);assert!(a.next_packet(now).unwrap().is_none());assert!(b.next_packet(now).unwrap().is_none());
}

#[test]
fn out_of_order_data_fills_original_preceding_data_chain_once(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 assert!(c.on_packet(data(0x9003,0,&[0xff,b'c']).expose_to_transport(),now).unwrap().is_empty());
 assert!(c.on_packet(data(0x9001,0,&[0xfe,b'a']).expose_to_transport(),now).unwrap().is_empty());
 let out=c.on_packet(data(0x9002,0,&[0xfe,b'b']).expose_to_transport(),now).unwrap();
 assert_eq!(out.len(),1);assert_eq!(out[0].expose_to_transport(),b"abc");assert!(!c.has_partial());assert_eq!(c.delivered_rx.unwrap() as u16,0x9003);
 assert!(c.on_packet(data(0x9002,0,&[0xfe,b'b']).expose_to_transport(),now).unwrap().is_empty());
 let retry=wire(0x9004,PacketKind::Skip,0,0,0,&[],false,&[0x40,2,0xfe,b'b']);
 assert!(c.on_packet(retry.expose_to_transport(),now).unwrap().is_empty());
}

#[test]
fn normal_non_data_gap_and_explicit_previous_data_escape_preserve_record_order(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 c.on_packet(probe(0x9001,0,0,&[],false).expose_to_transport(),now).unwrap();
 assert_eq!(c.on_packet(data(0x9002,1,&[0xff,b'a']).expose_to_transport(),now).unwrap()[0].expose_to_transport(),b"a");
 assert_eq!(c.on_packet(data(0x9028,31,&[0x90,2,0xff,b'b']).expose_to_transport(),now).unwrap()[0].expose_to_transport(),b"b");
}

#[test]
fn conflicting_duplicate_wire_or_retransmitted_root_is_terminal(){
 let now=Instant::now();for alias in [false,true]{
  let mut c=ready(now,0x9000);c.on_packet(data(0x9001,0,&[0xff,b'a']).expose_to_transport(),now).unwrap();
  let bad=if alias{wire(0x9002,PacketKind::Skip,0,0,0,&[],false,&[0x40,1,0xff,b'b'])}else{data(0x9001,0,&[0xff,b'b'])};
  assert_eq!(c.on_packet(bad.expose_to_transport(),now).err(),Some(ChannelError::Protocol));assert!(c.closed);
  assert!(c.next_packet(now).is_err());assert!(c.received.is_empty());
 }
}

#[test]
fn forked_predecessor_or_late_data_in_a_declared_non_data_gap_fails_closed(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 c.on_packet(data(0x9002,0,&[0xff,b'a']).expose_to_transport(),now).unwrap();
 assert_eq!(c.on_packet(data(0x9003,1,&[0xff,b'b']).expose_to_transport(),now).err(),Some(ChannelError::Protocol));
 let mut c=ready(now,0x9000);c.on_packet(data(0x9002,1,&[0xff,b'a']).expose_to_transport(),now).unwrap();
 assert_eq!(c.on_packet(data(0x9001,0,&[0xff,b'b']).expose_to_transport(),now).err(),Some(ChannelError::Protocol));
}

#[test]
fn native_skip_retransmission_has_new_wire_sequence_and_same_original_record(){
 let now=Instant::now();let mut c=ready(now,0x9000);let original=send(&mut c,b"a",now);
 let p=native_channel::decode_packet(HeaderMode::Basic,original.expose_to_transport()).unwrap();assert_eq!((p.sequence,p.data_gap),(0x4002,1));
 let retry=c.next_packet(now+Duration::from_millis(200)).unwrap().unwrap();let r=native_channel::decode_packet(HeaderMode::Basic,retry.expose_to_transport()).unwrap();
 assert_eq!((r.sequence,r.kind,r.data_gap),(0x4003,PacketKind::Skip,1));assert_eq!(r.body,[0x40,1,0xff,b'a']);
 let reconstructed=native_channel::original_packet(&r).unwrap();assert_eq!(reconstructed.sequence,p.sequence);assert_eq!(reconstructed.body,p.body);
 assert_eq!(c.roots.get(&(0x14002)).unwrap().deadline,now+Duration::from_secs(5));
}

#[test]
fn ack_of_retransmission_releases_only_its_captured_root_not_another_record(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);send(&mut c,b"b",now);
 let retry=c.next_packet(now+Duration::from_millis(200)).unwrap().unwrap();let r=native_channel::decode_packet(HeaderMode::Basic,retry.expose_to_transport()).unwrap();
 assert_eq!(r.sequence,0x4004);c.on_packet(probe(0x9001,r.sequence,0x4000,&[],false).expose_to_transport(),now+Duration::from_millis(200)).unwrap();
 assert!(c.roots.get(&0x14002).unwrap().acknowledged);assert!(!c.roots.get(&0x14003).unwrap().acknowledged);
}

#[test]
fn unknown_future_ack_anchor_cannot_apply_bitmap_or_fabricated_delivery(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);
 let fake=probe(0x9001,0x4003,0x4002,&[0xff],true);c.on_packet(fake.expose_to_transport(),now).unwrap();
 assert!(!c.roots.get(&0x14002).unwrap().acknowledged);assert!(c.roots.get(&0x14002).unwrap().body.is_some());
 assert!(c.next_packet(now+Duration::from_millis(200)).unwrap().is_some());
}

#[test]
fn selective_ack_holes_can_be_filled_when_highest_ack_does_not_advance(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);send(&mut c,b"b",now);
 c.on_packet(probe(0x9001,0x4003,0x4000,&[0],false).expose_to_transport(),now).unwrap();
 assert!(!c.roots.get(&0x14002).unwrap().acknowledged);assert!(c.roots.get(&0x14003).unwrap().acknowledged);
 c.on_packet(probe(0x9002,0x4003,0x4003,&[4],false).expose_to_transport(),now).unwrap();assert!(c.roots.is_empty());
}

#[test]
fn matched_cumulative_ack_obeys_actual_send_history_and_delivery_cursor(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);send(&mut c,b"b",now);
 c.on_packet(probe(0x9001,0x4003,0x4000,&[],true).expose_to_transport(),now).unwrap();
 assert_eq!(c.roots.len(),2);assert!(c.roots.values().all(|root|root.acknowledged&&root.body.is_none()));
 c.on_packet(probe(0x9002,0x4003,0x4003,&[],true).expose_to_transport(),now).unwrap();assert!(c.roots.is_empty());
 assert!(c.next_packet(now+Duration::from_secs(1)).unwrap().is_none());
}

#[test]
fn full_selective_receive_window_reacks_late_data_without_claiming_missing_packets(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 c.on_packet(data(0x9002,0,&[0xff,b'b']).expose_to_transport(),now).unwrap();
 let ack=c.next_packet(now).unwrap().unwrap();let ack=native_channel::decode_packet(HeaderMode::Basic,ack.expose_to_transport()).unwrap();
 assert!(!ack.inclusive_ack);let acknowledged=native_channel::acknowledged_sequences(&ack,0x8fff).unwrap();
 assert!(acknowledged.contains(&0x9000)&&acknowledged.contains(&0x9002));assert!(!acknowledged.contains(&0x9001));
 c.on_packet(data(0x9001,0,&[0xff,b'a']).expose_to_transport(),now).unwrap();
 let ack=c.next_packet(now).unwrap().unwrap();let ack=native_channel::decode_packet(HeaderMode::Basic,ack.expose_to_transport()).unwrap();
 assert_eq!(ack.ack_sequence,0x9002);assert!(native_channel::acknowledged_sequences(&ack,0x9002).unwrap().contains(&0x9001));
}

#[test]
fn receiver_u16_wrap_stays_in_one_original_incarnation(){
 let now=Instant::now();let mut c=ready(now,0xfffe);
 assert_eq!(c.on_packet(data(0xffff,0,&[0xff,b'a']).expose_to_transport(),now).unwrap()[0].expose_to_transport(),b"a");
 assert_eq!(c.on_packet(data(0,0,&[0xff,b'b']).expose_to_transport(),now).unwrap()[0].expose_to_transport(),b"b");
 assert_eq!(c.highest_rx,Some(0x20000));assert_eq!(c.delivered_rx,Some(0x20000));
}

#[test]
fn sender_wrap_allows_real_post_start_ack_zero_without_rpc_id_reuse(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 // Equivalent boundary state after previously delivered traffic; no public
 //reset/relabel API exposes this test-only cursor setup.
 c.next_tx=0x1ffff;c.last_data_tx=0x1fffd;c.last_ack=0x1fffd;c.sent.clear();
 let a=send(&mut c,b"a",now);let b=send(&mut c,b"b",now);
 assert_eq!(native_channel::decode_packet(HeaderMode::Basic,a.expose_to_transport()).unwrap().sequence,0xffff);
 assert_eq!(native_channel::decode_packet(HeaderMode::Basic,b.expose_to_transport()).unwrap().sequence,0);
 c.on_packet(probe(0x9001,0,0xfffd,&[],true).expose_to_transport(),now).unwrap();
 assert_eq!(c.roots.len(),2);assert!(c.roots.values().all(|root|root.acknowledged&&root.body.is_none()));
 c.on_packet(probe(0x9002,0,0,&[],true).expose_to_transport(),now).unwrap();assert!(c.roots.is_empty());
}

#[test]
fn header_budget_reserves_ack_gap_and_retransmission_bytes_before_segmentation(){
 let now=Instant::now();let mut c=ready(now,0x9000);c.mtu=128;c.body_budget=128-8-ACK_BYTES-2-2;
 c.queue_record(&[b'x';120],now+Duration::from_secs(5),now).unwrap();let mut packets=Vec::new();
 while let Some(p)=c.next_packet(now).unwrap(){assert!(p.expose_to_transport().len()<=128);packets.push(p);}
 assert_eq!(packets.len(),3);let mut records=Records::default();let mut out=Vec::new();
 for bytes in packets{let p=native_channel::decode_packet(HeaderMode::Basic,bytes.expose_to_transport()).unwrap();out.extend(records.on_payload(native_channel::previous_data(&p).unwrap().1).unwrap());}
 assert_eq!(out.len(),1);assert_eq!(out[0].expose_to_transport(),[b'x';120]);
 let retry=c.next_packet(now+Duration::from_millis(200)).unwrap().unwrap();assert!(retry.expose_to_transport().len()<=128);
}

#[test]
fn transmit_mtu_does_not_invent_a_smaller_peer_receive_mtu(){
 let now=Instant::now();let mut c=ready(now,0x9000);c.mtu=128;c.body_budget=52;let mut body=vec![0xff];body.extend_from_slice(&[b'x';1000]);
 let out=c.on_packet(data(0x9001,0,&body).expose_to_transport(),now).unwrap();assert_eq!(out[0].expose_to_transport().len(),1000);
}

#[test]
fn queue_overflow_is_atomic_and_does_not_consume_original_channel_sequence(){
 let now=Instant::now();let mut c=ready(now,0x9000);let next=c.next_tx;
 assert_eq!(c.queue_record(&vec![0;MAX_BYTES],now+Duration::from_secs(1),now),Err(ChannelError::Window));
 assert_eq!(c.next_tx,next);assert!(c.queued.is_empty());assert!(c.ready());send(&mut c,b"ok",now);
}

#[test]
fn pending_out_of_order_roots_are_bounded_before_payload_retention(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 for index in 2..=(MAX_PENDING as u16+1){assert!(c.on_packet(data(0x9000+index,0,&[0xff,b'x']).expose_to_transport(),now).unwrap().is_empty());}
 assert_eq!(c.received.len(),MAX_PENDING);
 assert_eq!(c.on_packet(data(0x9000+MAX_PENDING as u16+2,0,&[0xff,b'x']).expose_to_transport(),now).err(),Some(ChannelError::Window));assert!(c.received.is_empty());
}

#[test]
fn retry_probe_and_new_rtt_sample_never_renew_original_delivery_deadline(){
 let now=Instant::now();let mut c=ready(now,0x9000);c.queue_record(b"x",now+Duration::from_millis(500),now).unwrap();c.next_packet(now).unwrap();
 c.set_echoed_delay(Some(16000));c.on_packet(probe(0x9001,0,0,&[],false).expose_to_transport(),now+Duration::from_millis(400)).unwrap();
 assert_eq!(c.tick(now+Duration::from_millis(500)),Err(ChannelError::Timeout));assert!(c.closed);
}

#[test]
fn partial_record_and_out_of_order_gap_keep_their_first_admission_budgets(){
 let now=Instant::now();let mut c=ready(now,0x9000);c.on_packet(data(0x9001,0,&[0xfe,b'a']).expose_to_transport(),now).unwrap();
 c.on_packet(probe(0x9002,0,0,&[],false).expose_to_transport(),now+Duration::from_secs(1)).unwrap();
 assert_eq!(c.tick(now+Duration::from_secs(2)),Err(ChannelError::Timeout));
 let mut c=ready(now,0x9000);c.on_packet(data(0x9002,0,&[0xfe,b'b']).expose_to_transport(),now).unwrap();
 c.on_packet(data(0x9001,0,&[0xfe,b'a']).expose_to_transport(),now+Duration::from_millis(1900)).unwrap();
 assert!(c.received.is_empty());assert_eq!(c.partial_end,Some(now+Duration::from_secs(2)));
 assert_eq!(c.tick(now+Duration::from_secs(2)),Err(ChannelError::Timeout));
}

#[test]
fn repeated_start_does_not_reset_partial_cipher_record_or_startup_deadline(){
 let now=Instant::now();let mut c=ready(now,0x9000);c.on_packet(data(0x9001,0,&[0xfe,b'a']).expose_to_transport(),now).unwrap();
 let old=c.startup_end;c.on_packet(wire(0x9000,PacketKind::Start,0,0,0,&[],false,&[]).expose_to_transport(),now+Duration::from_secs(1)).unwrap();
 assert_eq!(c.startup_end,old);assert!(c.records.has_partial());assert_eq!(c.tick(now+Duration::from_secs(2)),Err(ChannelError::Timeout));
}

#[test]
fn native_missing_start_probe_rebuilds_only_once_before_any_data_and_keeps_deadline(){
 let now=Instant::now();let mut c=core(now);c.next_packet(now).unwrap();let end=c.startup_end;
 let reset=wire(0x4001,PacketKind::Probe,0,0x4001,0,&[0;4],false,&[]);
 c.on_packet(reset.expose_to_transport(),now).unwrap();let start=c.next_packet(now).unwrap().unwrap();
 assert_eq!(native_channel::decode_packet(HeaderMode::Basic,start.expose_to_transport()).unwrap().sequence,0x8001);assert_eq!(c.startup_end,end);
 assert_eq!(c.on_packet(reset.expose_to_transport(),now).err(),Some(ChannelError::PeerReset));assert!(c.closed);
}

#[test]
fn data_before_start_produces_exact_missing_incarnation_probe_not_plaintext(){
 let now=Instant::now();let mut c=core(now);c.next_packet(now).unwrap();
 assert!(c.on_packet(data(0x9001,0,&[0xff,b'x']).expose_to_transport(),now).unwrap().is_empty());
 assert_eq!(c.next_packet(now).unwrap().unwrap().expose_to_transport(),[0x90,1,0x60,4,0x90,1,0,0,0,0,0,0]);assert!(!c.ready());
}

#[test]
fn changed_start_or_native_reset_after_ready_closes_without_business_replay(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"private",now);
 let changed=wire(0x9002,PacketKind::Start,0,0,0,&[],false,&[]);
 assert_eq!(c.on_packet(changed.expose_to_transport(),now).err(),Some(ChannelError::PeerReset));assert!(c.roots.is_empty());
 let mut c=ready(now,0x9000);let reset=wire(0x4001,PacketKind::Probe,0,0x4001,0,&[0;4],false,&[]);
 assert_eq!(c.on_packet(reset.expose_to_transport(),now).err(),Some(ChannelError::PeerReset));assert!(!c.ready());
}

#[test]
fn bad_ack_mask_data_prefix_fec_and_probe_are_rejected_without_record_output(){
 let now=Instant::now();let cases=[vec![0x90,1,0x20,0x7f,0,0,0,0],
  data(0x9001,31,&[0]).expose_to_transport().to_vec(),wire(0x9001,PacketKind::Fec,0,0,0,&[],false,&[1,2]).expose_to_transport().to_vec(),
  wire(0x9001,PacketKind::Probe,0,0,0,&[],false,&[]).expose_to_transport().to_vec()];
 for bytes in cases{let mut c=ready(now,0x9000);assert!(c.on_packet(&bytes,now).is_err());assert!(c.closed);assert!(c.received.is_empty());}
}

#[test]
fn native_probe_receive_limit_hint_cannot_grow_queues_or_renew_timers(){
 let now=Instant::now();let mut c=ready(now,0x9000);let mut body=PROBE_MAGIC.to_vec();body.extend_from_slice(&[0,0xff,0xff,0xff,0xff,0,0,0,0,0,0xff]);
 c.on_packet(wire(0x9001,PacketKind::Probe,0,0,0,&[],false,&body).expose_to_transport(),now).unwrap();
 assert!(c.queued.is_empty());assert!(!c.ack_pending);assert_eq!(c.partial_timeout,Duration::from_secs(2));
}

#[test]
fn rollback_local_ordinal_exhaustion_and_ambiguous_half_space_fail_closed(){
 let now=Instant::now();let mut c=ready(now,0x9000);assert_eq!(c.tick(now-Duration::from_millis(1)),Err(ChannelError::ClockRollback));
 let mut c=ready(now,0x9000);c.next_tx=u64::MAX;c.last_data_tx=u64::MAX-1;c.queue_record(b"x",now+Duration::from_secs(1),now).unwrap();
 assert_eq!(c.next_packet(now).err(),Some(ChannelError::Window));assert!(c.closed);assert_eq!(extend(0x8000,0x10000),Err(ChannelError::Window));
}

#[test]
fn malformed_second_record_suppresses_valid_first_record_in_same_input(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 // First BE16 record is valid; the following empty record is not. No staged
 //record leaves on_packet after the group fails.
 assert!(c.on_packet(data(0x9001,0,&[0,1,b'a',0,0]).expose_to_transport(),now).is_err());assert!(c.closed);
}

#[test]
fn constructor_deadlines_and_debug_do_not_allow_unbounded_or_exposed_state(){
 let now=Instant::now();assert!(ChannelCore::new(0,80,now+Duration::from_secs(1),Duration::from_secs(1),now).is_err());
 assert!(ChannelCore::new(0,1460,now+Duration::from_secs(31),Duration::from_secs(1),now).is_err());
 assert!(ChannelCore::new(0,1460,now+Duration::from_secs(1),Duration::ZERO,now).is_err());
 let mut c=ready(now,0x9000);send(&mut c,b"private-channel-value",now);let debug=format!("{c:?}");
 assert!(debug.contains("<redacted>"));assert!(!debug.contains("private-channel-value"));c.close();assert!(!c.ready());assert!(c.next_wake().is_none());
}

#[test]
fn crossing_data_edges_are_rejected_before_buffered_records_can_drain(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 c.on_packet(data(0x9002,0,&[0xff,b'a']).expose_to_transport(),now).unwrap();
 assert_eq!(c.on_packet(data(0x9003,2,&[0xff,b'b']).expose_to_transport(),now).err(),Some(ChannelError::Protocol));
 // Reverse arrival while Start is waiting for its matched ACK: no record
 //can have escaped before either of the contradictory edges is checked.
 let mut c=core(now);c.next_packet(now).unwrap();
 c.on_packet(wire(0x9000,PacketKind::Start,0,0,0,&[],false,&[]).expose_to_transport(),now).unwrap();
 assert!(c.on_packet(data(0x9003,2,&[0xff,b'b']).expose_to_transport(),now).unwrap().is_empty());
 assert_eq!(c.on_packet(data(0x9002,0,&[0xff,b'a']).expose_to_transport(),now).err(),Some(ChannelError::Protocol));
}

#[test]
fn reserved_zero_mask_probe_pattern_is_not_downgraded_to_ordinary_probe_by_extra_body(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 // F53EE8 passes the still-prefixed mask to F562E0. Four zero mask bytes
 //with header0460 are reserved even if trailing bytes look like a Probe.
 assert_eq!(c.on_packet(probe(0x9001,0,0,&[0;4],false).expose_to_transport(),now).err(),Some(ChannelError::Protocol));
 let mut c=ready(now,0x9000);
 // Bit80 makes a different native marker; it remains an ordinary Probe.
 assert!(c.on_packet(probe(0x9001,0,0,&[0;4],true).expose_to_transport(),now).unwrap().is_empty());assert!(c.ready());
}

#[test]
fn native_inclusive_prefix_is_not_a_tcp_cumulative_ack_of_older_selective_holes(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);send(&mut c,b"b",now);
 c.on_packet(probe(0x9001,0x4003,0x4000,&[0],false).expose_to_transport(),now).unwrap();
 c.on_packet(probe(0x9002,0x4003,0x4000,&[],true).expose_to_transport(),now).unwrap();
 assert!(!c.roots.get(&0x14002).unwrap().acknowledged);assert!(c.roots.get(&0x14002).unwrap().body.is_some());
 assert!(c.roots.get(&0x14003).unwrap().acknowledged);
 c.on_packet(probe(0x9003,0x4003,0x4000,&[4],false).expose_to_transport(),now).unwrap();
 assert_eq!(c.roots.len(),2);assert!(c.roots.values().all(|root|root.acknowledged&&root.body.is_none()));
}

#[test]
fn matched_ack_anchor_cannot_fabricate_delivery_of_a_later_original_root(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);send(&mut c,b"b",now);
 assert_eq!(c.on_packet(probe(0x9001,0x4002,0x4003,&[],false).expose_to_transport(),now).err(),Some(ChannelError::Protocol));assert!(c.closed);
}

#[test]
fn nested_skip_distances_cannot_wrap_away_a_whole_receive_incarnation(){
 let now=Instant::now();let mut c=ready(now,0x9000);
 let body=[0xbf,0xff,0xbf,0xff,0xbf,0xff,0xbf,0xff,0x40,4,0xff,b'x'];
 let bytes=wire(0x9001,PacketKind::Skip,0,0,0,&[],false,&body);
 let packet=native_channel::decode_packet(HeaderMode::Basic,bytes.expose_to_transport()).unwrap();
 assert_eq!(native_channel::original_packet_distance(&packet).unwrap().1,65536);
 assert_eq!(c.on_packet(bytes.expose_to_transport(),now).err(),Some(ChannelError::Window));assert!(c.closed);
}

#[test]
fn extended_transmit_reference_bounds_precede_narrowing_and_ack_only_emission(){
 let now=Instant::now();let mut c=ready(now,0x9000);send(&mut c,b"a",now);c.next_tx=0x14002+65537;
 assert_eq!(c.next_packet(now+Duration::from_millis(200)).err(),Some(ChannelError::Window));
 let mut c=ready(now,0x9000);c.next_tx=c.last_data_tx+65537;c.queue_record(b"x",now+Duration::from_secs(1),now).unwrap();
 assert_eq!(c.next_packet(now).err(),Some(ChannelError::Window));
 let mut c=ready(now,0x9000);c.next_tx=c.last_data_tx+HISTORY;c.ack_pending=true;
 assert_eq!(c.next_packet(now).err(),Some(ChannelError::Window));
}

#[test]
fn receive_retention_budget_includes_existing_ordered_partial_and_future_gap_bodies(){
 let now=Instant::now();let mut c=ready(now,0x9000);let mut body=vec![0xfe];body.extend_from_slice(&vec![b'x';MAX_BYTES/16]);
 // Accumulate through legal (<128KiB) lower-layer packets rather than an
 //oversized fixture that would fail before reaching aggregate accounting.
 for index in 1..=9{c.on_packet(data(0x9000+index,0,&body).expose_to_transport(),now).unwrap();}
 assert!(c.records.retained_bytes()>MAX_BYTES/2);
 // DATA10 is missing. Six later legal packets fit; the seventh crosses the
 //aggregate partial+pending bound even though each component still fits.
 for index in 11..=16{c.on_packet(data(0x9000+index,0,&body).expose_to_transport(),now).unwrap();}
 assert_eq!(c.on_packet(data(0x9011,0,&body).expose_to_transport(),now).err(),Some(ChannelError::Window));
 assert_eq!(c.records.retained_bytes(),0);assert!(c.received.is_empty());
}

#[test]
fn empty_contiguous_start_waits_for_startup_not_an_invented_data_gap_deadline(){
 let now=Instant::now();let mut c=core(now);c.next_packet(now).unwrap();
 c.on_packet(wire(0x9000,PacketKind::Start,0,0,0,&[],false,&[]).expose_to_transport(),now).unwrap();
 assert!(c.gap_end.is_none());assert!(c.partial_end.is_none());assert!(!c.ready());
 assert_eq!(c.tick(now+Duration::from_secs(3)),Ok(()));assert_eq!(c.tick(now+Duration::from_secs(10)),Err(ChannelError::Timeout));
}

#[test]
fn uninitialised_ack_zero_halfway_from_pre_send_reset_is_ignored_not_fatal(){
 let now=Instant::now();let mut c=core(now);
 let reset=wire(0x9001,PacketKind::Probe,0,0x9001,0,&[0;4],false,&[]);
 c.on_packet(reset.expose_to_transport(),now).unwrap();let start=c.next_packet(now).unwrap().unwrap();
 assert_eq!(native_channel::decode_packet(HeaderMode::Basic,start.expose_to_transport()).unwrap().sequence,0x8000);
 assert!(c.on_packet(wire(0x9000,PacketKind::Start,0,0,0,&[],false,&[]).expose_to_transport(),now).unwrap().is_empty());
 assert!(!c.closed);assert!(!c.start_acked);assert!(!c.ready());
}

#[test]
fn delayed_channel_admission_uses_original_receipt_for_partial_and_gap_expiry(){
 let now=Instant::now();for sequence in [0x9001,0x9002]{
  let mut c=ready(now,0x9000);let packet=data(sequence,0,&[0xfe,b'x']);
  assert_eq!(c.on_packet_admitted(packet.expose_to_transport(),now,now+Duration::from_secs(2)).err(),Some(ChannelError::Timeout));assert!(c.closed);
 }
}

#[test]
fn future_receipt_timestamp_is_rejected_without_delivering_a_complete_record(){
 let now=Instant::now();let mut c=ready(now,0x9000);let packet=data(0x9001,0,&[0xff,b'x']);
 assert_eq!(c.on_packet_admitted(packet.expose_to_transport(),now+Duration::from_millis(1),now).err(),Some(ChannelError::ClockRollback));assert!(c.closed);
}

#[test]
fn hard_expiration_is_separate_from_retry_wake_and_keeps_old_obligations(){
 let now=Instant::now();let initial=core(now);assert_eq!(initial.hard_deadline(),Some(now+Duration::from_secs(10)));
 let mut c=ready(now,0x9000);assert_eq!(c.hard_deadline(),None);
 c.queue_record(b"x",now+Duration::from_secs(5),now).unwrap();c.next_packet(now).unwrap();
 assert_eq!(c.hard_deadline(),Some(now+Duration::from_secs(5)));assert_eq!(c.next_wake(),Some(now+Duration::from_millis(200)));
 c.set_echoed_delay(Some(16000));assert_eq!(c.hard_deadline(),Some(now+Duration::from_secs(5)));
 c.on_packet(data(0x9001,0,&[0xfe,1]).expose_to_transport(),now).unwrap();assert_eq!(c.hard_deadline(),Some(now+Duration::from_secs(2)));
 c.close();assert_eq!(c.hard_deadline(),None);
}
