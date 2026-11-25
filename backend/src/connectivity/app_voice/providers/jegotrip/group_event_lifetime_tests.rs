//! Additional Actions-only exact-holder, epoch, SQL-fence and resource tests.
use super::*;
fn batch_event(f:&mut Fixture)->Box<IncomingGroupEvent>{
    let request=f.backend.prepare_swap(0,&[]).unwrap();let outer=decode_request(&request.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let data=n::rpc_batch::read_payload(&mut r).unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,data.fields[0],data.fields[1],data.fields[2]]{w.u32(word).unwrap();}w.u32(1).unwrap();w.i32(1).unwrap();w.bytes(n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();w.bytes(&payload(Some(b"1-2"),0,1,b"a")).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"}]).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();
    let events=f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,w.finish().expose_to_transport()),1).unwrap();events.into_iter().find_map(|e|if let OfflineEvent::GroupEvent(e)=e{Some(e)}else{None}).unwrap()
}
#[test]fn group_event_result_encoding_is_exact_original_bool_footer_and_rpc_envelope(){
    for accepted in [false,true]{let a=n::group_event_packets::result(accepted).unwrap();let b=n::rpc_incoming::encode_online_message_result(accepted).unwrap();assert_eq!(a.expose_to_transport(),b.expose_to_transport());let a=n::group_event_packets::direct_reply(19,accepted).unwrap();let b=n::rpc_envelope::encode_reply(19,0,b.expose_to_transport()).unwrap();assert_eq!(a.expose_to_transport(),b.expose_to_transport());}assert!(n::group_event_packets::direct_reply(0,true).is_err());
}
#[test]fn group_event_direct_packet_itself_keeps_notice_budget_after_waiter_and_protocol_close(){
    let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&e);let Some(n::offline_backend::CallCompletion::Direct(mut frames))=f.backend.complete_group_event(e.ticket(),Some(&p)).unwrap()else{panic!("direct")};let packet=frames.packets.pop().unwrap();assert!(packet.group_event_packet_current(Instant::now()));drop(frames);drop(p);drop(e);f.backend.close();assert!(f.scope.native_rpc_resources().0>0);assert!(!packet.group_event_packet_current(Instant::now()));drop(packet);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]fn group_event_durable_duplicate_must_acquire_its_own_notice_proof_and_deadline(){
    let mut f=current(true);let first=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&first);let later=event(&mut f,2,Some(b"1-2"),0,1,b"a");assert_eq!(first.ticket().fingerprint(),later.ticket().fingerprint());assert!(!p.authorizes(later.ticket(),Instant::now()),"same DATA cannot renew the old receipt deadline");assert!(f.backend.complete_group_event(later.ticket(),Some(&p)).unwrap().is_none());let own=commit(&f,&later);assert!(own.authorizes(later.ticket(),Instant::now()));assert!(!own.authorizes(later.ticket(),later.ticket().deadline()));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_direct_generic_sms_completion_cannot_mint_a_positive_relation_receipt(){
    let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let ImReplySource::Direct(t)=e.ticket().source()else{panic!("direct")};assert!(f.backend.complete_direct_message(t,true).is_err());assert!(history(&f).is_empty());assert_eq!(result(f.backend.complete_group_event(e.ticket(),None).unwrap()),[0;6]);
}
#[test]fn group_event_batch_result_retains_exact_epoch_proof_and_detached_packet_after_ack(){
    let mut f=current(true);let e=batch_event(&mut f);let ImReplySource::Batch(inner)=e.ticket().source()else{panic!("batch")};assert!(f.backend.complete_message(inner,true).is_err());let p=commit(&f,&e);assert!(matches!(f.backend.complete_group_event(e.ticket(),Some(&p)).unwrap(),Some(n::offline_backend::CallCompletion::BatchQueued)));assert_eq!(f.backend.im_write_guard().0.len(),1);
    let mut next=f.backend.prepare_swap(2,&[]).unwrap();let outer=decode_request(&next.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let data=n::rpc_batch::read_payload(&mut r).unwrap();assert_eq!(data.results[&1].result_stream,[0,1,0,0,0,0]);let ack=n::runtime_backend::tests::swap_result(None,true,data.fields);f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,&ack),3).unwrap();assert!(f.backend.im_write_guard().0.is_empty());
    let packet=next.packets.pop().unwrap();drop(next);drop(p);drop(e);f.backend.close();assert!(f.scope.native_rpc_resources().0>0);drop(packet);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_retired_batch_positive_is_not_reencoded_as_negative_or_replayed(){
    let mut f=current(true);let e=batch_event(&mut f);let p=commit(&f,&e);f.backend.complete_group_event(e.ticket(),Some(&p)).unwrap();f.gate.retire();assert!(f.backend.prepare_swap(2,&[]).is_err());assert!(!f.scope.is_open());assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_im_policy_receivers_and_empty_private_config_do_not_imply_event_installation(){
    let f=current(false);let stored=f.temp.database.with_conn(|c|account_store::load(c,f.run.line_id())).unwrap().unwrap();let raw=stored.config.encode_private().unwrap();assert!(!std::str::from_utf8(&raw).unwrap().contains("relation_events"));
    let mut f=current(true);assert!(f.backend.arm_relation_events(NativeRelationEventsConfig{group_updates:true}).is_err());assert!(f.scope.is_open());
}
#[test]fn group_event_native_unknown_write_and_orphan_intent_stay_unknown_after_inbound_commit(){
    let mut f=current(true);f.temp.database.with_conn(|c|{c.execute("INSERT INTO app_voice_relation_writes(line_id,revision,user_nonce,intent_hash,identity_key,run_nonce,scope_epoch,target_kind,additions,updates,removals,state,request_id,updated_at) VALUES(?1,1,?2,zeroblob(32),zeroblob(32),?3,'fixture-old-epoch','buddy',1,0,0,0,7,'2026-01-01T00:00:00Z')",rusqlite::params![f.run.line_id(),"a".repeat(32),"b".repeat(32)]).map_err(|_|"fixture")?;c.execute("INSERT INTO app_voice_relation_intents(line_id,revision,user_nonce,body_hash,run_nonce,claim_nonce,configuration_hash,target_kind,additions,updates,removals,state,updated_at) VALUES(?1,1,?2,zeroblob(32),?3,?4,zeroblob(32),'buddy',1,0,0,0,'2026-01-01T00:00:00Z')",rusqlite::params![f.run.line_id(),"c".repeat(32),"d".repeat(32),"e".repeat(32)]).map_err(|_|"fixture")}).unwrap();
    let e=event(&mut f,1,None,0,1,b"a");commit(&f,&e);let states:(i64,i64)=f.temp.database.with_conn(|c|c.query_row("SELECT (SELECT state FROM app_voice_relation_writes),(SELECT state FROM app_voice_relation_intents)",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")).unwrap();assert_eq!(states,(0,0));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_revision_or_runtime_lease_loss_blocks_sql_even_with_current_wire_identity(){
    for revision in [false,true]{let mut f=current(true);let e=event(&mut f,1,None,0,1,b"a");let original=config(&f);f.temp.database.with_conn(|c|c.execute(if revision{"UPDATE app_voice_accounts SET revision=2"}else{"DELETE FROM app_voice_runtime_leases"},[]).map_err(|_|"fixture")).unwrap();assert!(group_event_inbox::commit(&f.temp.database,f.run.clone(),&e,f.gate.clone(),&original).is_err());assert!(history(&f).is_empty());}
}
#[test]fn group_event_shared_native_budget_refusal_allocates_no_durable_row_or_new_receipt(){
    let mut f=current(true);let used=f.scope.native_rpc_resources().1;let hold=f.scope.reserve_native_rpc(n::rpc_stream::MAX_STREAM_BYTES-used).unwrap();let body=payload(Some(b"1-2"),0,1,b"a");let magic=1i32.to_be_bytes();let bytes=n::rpc_envelope::encode_request(1,&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"__magic",value:&magic}],n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&body).unwrap();let mut packet=vec![0];packet.extend_from_slice(bytes.expose_to_transport());assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&packet,0).is_err());assert!(history(&f).is_empty());drop(hold);f.backend.close();assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
fn channel_ready(now:Instant)->n::reliable_channel::ChannelCore{
    use n::native_channel::{self,HeaderMode,PacketKind,ChannelPacket};
    let mut core=n::reliable_channel::ChannelCore::new(0,1460,now+Duration::from_secs(10),Duration::from_secs(2),now).unwrap();let start=core.next_packet(now).unwrap().unwrap();let sent=native_channel::decode_packet(HeaderMode::Basic,start.expose_to_transport()).unwrap();
    let peer=native_channel::encode_packet(HeaderMode::Basic,&ChannelPacket{timestamp:None,sequence:0x9000,kind:PacketKind::Start,data_gap:0,ack_sequence:sent.sequence,delivered_sequence:sent.sequence,inclusive_ack:false,ack_mask:&[],body:&[]}).unwrap();core.on_packet(peer.expose_to_transport(),now).unwrap();assert!(core.ready());core.next_packet(now).unwrap().unwrap();core
}
#[test]fn group_event_lower_channel_retry_owns_same_proof_until_acked_or_actual_last_packet_drop(){
    let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&e);let retained=n::group_event_packets::GroupEventPackets::capture(&f.scope,std::iter::once((e.ticket(),Some(&p)))).unwrap();let now=Instant::now();let mut core=channel_ready(now);core.queue_record_retained(b"original encoded record",now+Duration::from_secs(3),now,Some(retained.clone())).unwrap();let original=core.next_packet(now).unwrap().unwrap();assert!(original.group_event_packet_current(now));drop(retained);drop(original);drop(e);drop(p);
    let retry=core.next_packet(now+Duration::from_millis(200)).unwrap().unwrap();assert!(retry.group_event_retention().is_some());assert!(retry.group_event_packet_current(now));core.close();f.backend.close();assert!(f.scope.native_rpc_resources().0>0,"actual retry packet remains charged after core/socket cleanup");drop(retry);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]fn group_event_lower_channel_parent_retirement_blocks_resend_after_first_submission(){
    let mut f=current(true);let e=event(&mut f,1,None,0,1,b"a");let p=commit(&f,&e);let retained=n::group_event_packets::GroupEventPackets::capture(&f.scope,std::iter::once((e.ticket(),Some(&p)))).unwrap();let now=Instant::now();let mut core=channel_ready(now);core.queue_record_retained(b"original encoded record",now+Duration::from_secs(3),now,Some(retained)).unwrap();let packet=core.next_packet(now).unwrap().unwrap();f.gate.retire();assert!(!packet.group_event_packet_current(Instant::now()));assert_eq!(core.next_packet(now+Duration::from_millis(200)).err(),Some(n::reliable_channel::ChannelError::Closed));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_lower_channel_same_revision_configuration_change_blocks_retry_and_saved_receipt(){
    let mut f=current(true);let e=event(&mut f,1,None,0,1,b"a");let p=commit(&f,&e);let retained=n::group_event_packets::GroupEventPackets::capture(&f.scope,std::iter::once((e.ticket(),Some(&p)))).unwrap();let now=Instant::now();let mut core=channel_ready(now);core.queue_record_retained(b"original encoded record",now+Duration::from_secs(3),now,Some(retained)).unwrap();core.next_packet(now).unwrap().unwrap();f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET private_config=X'00'",[]).map_err(|_|"fixture")).unwrap();assert!(!p.authorizes(e.ticket(),Instant::now()));assert_eq!(core.next_packet(now+Duration::from_millis(200)).err(),Some(n::reliable_channel::ChannelError::Closed));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_receipt_rechecks_original_deadline_after_waiting_for_database(){
    let mut f=current_timeout(true,100);let e=event(&mut f,1,None,0,1,b"a");let p=commit(&f,&e);let ticket=e.ticket().captured_copy();let deadline=ticket.deadline();let db=f.temp.database.clone();let(held,wait)=std::sync::mpsc::channel();let(release,released)=std::sync::mpsc::channel();
    let blocker=std::thread::spawn(move||db.with_conn(|_|{held.send(()).unwrap();released.recv().unwrap();Ok(())}).unwrap());wait.recv().unwrap();
    let(begin,begun)=std::sync::mpsc::channel();let check=std::thread::spawn(move||{let original=Instant::now();begin.send(()).unwrap();p.authorizes(&ticket,original)});begun.recv().unwrap();
    std::thread::sleep(deadline.saturating_duration_since(Instant::now())+Duration::from_millis(10));release.send(()).unwrap();blocker.join().unwrap();assert!(!check.join().unwrap(),"stale pre-SQL now cannot authorize after original expiry");assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_installation_does_not_add_new_token_validation_to_unrelated_moremessage(){
    let mut f=current(true);let raw=more(b"P2P/1_2",7,b"Text",br#"{"text":"unchanged IM"}"#);let e=f.receive(1,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&raw,Some(b"unrelated-metadata"));assert!(matches!(e.as_slice(),[OfflineEvent::Im(_)]));assert!(history(&f).is_empty());assert!(f.scope.is_open());
}
