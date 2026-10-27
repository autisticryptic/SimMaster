//! Actions-only SQLite boundaries with pure original protocol capabilities.
//! Actual result WRITE/ACK/consumer FIFO is covered by the native manager tests.
use super::*;
use super::super::{account_inbox::InboxScope, account_runtime::{local_nonce,test_quiescence},
    account_test_support::{config,TempDatabase}, arc_session::ArcLoginInputs,
    offline_backend::{OfflineBackend,OfflineEvent,ScheduledIncomingMessage}, rpc_stream::{Writer,RpcProperty},
    rpc_envelope::{encode_reply,decode_request}, session::HostingSession,
    tls_engine::tests::Certificates, transport_packets::{encode_call_packets,CallAssembler,CallKind}};

struct Fixture {
    temp:TempDatabase,line:String,nonce:String,owner:AttemptOwner,scope:RpcConnectionScope,
    context:InboxScope,engine:OfflineBackend,clock:u64,token:Option<account_runs::RunToken>,
}
fn unframe(packets:&super::super::arc_protocol::OwnedCallPackets)->super::super::transport_packets::CallMessage {
    let mut assembler=CallAssembler::default();let mut result=None;
    for packet in &packets.packets {result=assembler.on_packet(packet.expose_to_transport()).unwrap();}result.unwrap()
}
impl Fixture {
    fn new()->Self {
        let temp=TempDatabase::new();let cert=Certificates::generate("DNS:fixture.invalid");let mut imported=config(&cert.root);
        imported.profile.incoming.as_mut().unwrap().receiver_kind=b"Sip.Request".to_vec();let line=format!("app-account-{}",local_nonce().unwrap());
        temp.database.with_conn(|conn|account_store::create(conn,&line,&imported,100)).unwrap();
        let mut session=HostingSession::new(line.clone());session.import(imported.credentials,100).unwrap();let owner=session.begin_login(100).unwrap();
        let mut engine=OfflineBackend::new(owner.clone());let scope=engine.connection_scope();
        let login=engine.prepare_login(ArcLoginInputs{typed_identity:b"username:fixture",domain_name:b"fixture.invalid",selected_session_id:b"selector",
            phone_login_token:b"pwd",application_key:b"key",domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"",client_properties:&[]},&[]).unwrap();
        let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-token").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();
        let bytes=encode_reply(login.call_id,0,w.finish().expose_to_transport()).unwrap();
        for packet in encode_call_packets(CallKind::Reply,bytes.expose_to_transport(),1024).unwrap(){engine.on_plaintext_packet(&owner,&scope,packet.expose_to_transport(),0).unwrap();}
        engine.configure(7,Some(super::super::rpc_incoming::IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"Sip.Request"})).unwrap();
        let nonce=local_nonce().unwrap();let token=temp.database.with_conn(|conn|account_runs::register_runtime(conn,&line,1,&nonce)).unwrap();
        let context=InboxScope::original(&line,1,&nonce,&owner,&scope).unwrap().with_receiver(super::super::rpc_incoming::IncomingBinding{
            object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"Sip.Request"}).unwrap();
        Self{temp,line,nonce,owner,scope,context,engine,clock:0,token:Some(token)}
    }
    fn feed(&mut self,requests:&[(i32,&[u8])],ack:bool)->Vec<OfflineEvent> {
        self.clock+=1;let swap=self.engine.prepare_swap(self.clock,&[]).unwrap();let packet=unframe(&swap);let request=decode_request(packet.payload.expose_to_transport()).unwrap();
        let mut reader=super::super::rpc_stream::Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let payload=super::super::rpc_batch::read_payload(&mut reader).unwrap();
        let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,7,if ack{payload.fields[1]}else{0},if ack{payload.fields[2]}else{0}]{w.u32(word).unwrap();}
        w.u32(requests.len() as u32).unwrap();
        for &(sequence,body) in requests {
            let mut args=Writer::versioned().unwrap();args.bytes(b"Sip.Request").unwrap();args.properties(&[RpcProperty{key:b"Sip.Message.From",value:b"fixture-original-sender"}]).unwrap();args.bytes(body).unwrap();
            w.i32(sequence).unwrap();w.bytes(super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();w.bytes(args.finish().expose_to_transport()).unwrap();
            w.properties(&[RpcProperty{key:b"id",value:b"#fixture-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:&sequence.to_be_bytes()}]).unwrap();
        }
        w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();let bytes=encode_reply(swap.call_id,0,w.finish().expose_to_transport()).unwrap();
        let mut events=Vec::new();for packet in encode_call_packets(CallKind::Reply,bytes.expose_to_transport(),1024).unwrap(){events.extend(self.engine.on_plaintext_packet(&self.owner,&self.scope,packet.expose_to_transport(),self.clock).unwrap());}events
    }
    fn step(&mut self,requests:&[(i32,&[u8])])->Result<Vec<ScheduledIncomingMessage>,&'static str> {
        let events=self.feed(requests,true);let mut retained=Vec::new();
        for event in events {
            match event {
                OfflineEvent::Message(message)=>{
                    self.context.commit_batch(&self.temp.database,&message)?;
                    assert!(self.engine.complete_message(message.ticket(),true).unwrap());
                    // Pure fixture only: mirrors the queued-state update, not a
                    // claim that this helper physically wrote a result.
                    let changed=self.temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_inbox SET reply_state='batch_queued'
                        WHERE line_id=?1 AND run_nonce=?2 AND source_kind='batch' AND peer_epoch=9 AND sequence=?3",
                        params![self.line,self.nonce,message.ticket().sequence()]).map_err(|_|STORAGE))?;assert_eq!(changed,1);
                    retained.push(message);
                }
                OfflineEvent::BatchRetirement(proof)=>apply(&self.temp.database,&self.line,1,&self.nonce,&self.owner,&self.scope,&proof)?,
                _=>panic!("original batch-only fixture"),
            }
        }
        Ok(retained)
    }
    fn count(&self,table:&str)->i64 {self.temp.database.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap()}
    fn proof(&mut self)->BatchReplayRetirement {self.feed(&[],true).into_iter().find_map(|event|if let OfflineEvent::BatchRetirement(proof)=event{Some(proof)}else{None}).unwrap()}
}

#[test]
fn rolling_2300_batch_sms_compact_copies_and_bounded_sparse_ids_without_losing_history() {
    let mut f=Fixture::new();
    for sequence in 1..=2300 {f.step(&[(sequence,b"fixture sustained SMS")]).unwrap();assert!(f.count("app_voice_inbox")<=129);assert!(f.count("app_voice_batch_replay_ids")<=2048);}
    f.step(&[]).unwrap();assert_eq!(f.count("app_voice_inbox"),128);assert_eq!(f.count("sms_messages"),2300);assert_eq!(f.count("app_voice_sms_projection"),2300);
    assert_eq!(f.count("app_events"),4600);assert!(f.engine.connection_scope().is_open());
    let through:i32=f.temp.database.with_conn(|conn|conn.query_row("SELECT through_sequence FROM app_voice_batch_replay_fences",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert!(through>0);
    assert!(f.temp.database.with_conn(|conn|retired(conn,&f.line,1,&f.nonce,9,1)).unwrap());
}
#[test]
fn sparse_original_deleted_ticket_and_repeated_batch_cannot_resurrect_sms_or_notification() {
    let mut f=Fixture::new();let first=f.step(&[(1,b"original deleted text")]).unwrap().remove(0);
    f.temp.database.with_conn(|conn|conn.execute("DELETE FROM sms_messages WHERE line_id=?1",[&f.line]).map_err(|_|STORAGE)).unwrap();
    for sequence in 2..=140 {f.step(&[(sequence,b"later text")]).unwrap();}f.step(&[]).unwrap();
    assert_eq!(f.context.commit_batch(&f.temp.database,&first).err(),Some("app_account_inbox_batch_retired"));
    assert!(f.step(&[(1,b"original deleted text")]).unwrap().is_empty());assert_eq!(f.count("sms_messages"),139);assert_eq!(f.count("app_events"),280);
    assert_eq!(f.count("app_voice_notification_jobs"),0);
    f.engine.close();let proof=test_quiescence(f.token.take().unwrap());f.temp.database.with_conn(|conn|account_runs::release_and_compact(conn,&proof)).unwrap();
    assert_eq!(f.count("app_voice_batch_replay_ids"),0);assert_eq!(f.count("app_voice_batch_replay_fences"),0);assert_eq!(f.count("app_voice_inbox"),0);assert_eq!(f.count("sms_messages"),139);
}
#[test]
fn failed_post_ack_compaction_rolls_back_new_tombstone_watermark_and_detachment() {
    let mut f=Fixture::new();for sequence in 1..=129 {f.step(&[(sequence,b"durable original")]).unwrap();}
    assert_eq!(f.count("app_voice_batch_replay_ids"),128);
    f.temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_batch_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|STORAGE)).unwrap();
    assert_eq!(f.step(&[]).err(),Some(STORAGE));assert_eq!(f.count("app_voice_batch_replay_ids"),128);assert_eq!(f.count("app_voice_inbox"),129);assert_eq!(f.count("sms_messages"),129);
    let detached:i64=f.temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_sms_projection WHERE envelope_retained=0",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(detached,0);
    let poll=f.engine.prepare_swap(200,&[]).unwrap();let packet=unframe(&poll);let request=decode_request(packet.payload.expose_to_transport()).unwrap();
    let mut reader=super::super::rpc_stream::Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();
    assert!(super::super::rpc_batch::read_payload(&mut reader).unwrap().results.is_empty(),"acknowledged result was not queued for retry");
}
#[test]
fn foreign_closed_revision_or_missing_lease_proof_cannot_retire_original_rows() {
    let mut f=Fixture::new();f.step(&[(1,b"original")]).unwrap();let mut other=Fixture::new();other.step(&[(1,b"foreign")]).unwrap();let foreign=other.proof();
    assert_eq!(apply(&f.temp.database,&f.line,1,&f.nonce,&f.owner,&f.scope,&foreign),Err(SCOPE));
    let proof=f.proof();assert_eq!(apply(&f.temp.database,&f.line,2,&f.nonce,&f.owner,&f.scope,&proof),Err(SCOPE));
    f.temp.database.with_conn(|conn|conn.execute("DELETE FROM app_voice_runtime_leases WHERE line_id=?1",[&f.line]).map_err(|_|STORAGE)).unwrap();
    assert_eq!(apply(&f.temp.database,&f.line,1,&f.nonce,&f.owner,&f.scope,&proof),Err(SCOPE));assert_eq!(f.count("app_voice_batch_replay_ids"),0);assert_eq!(f.count("app_voice_inbox"),1);
    f.scope.close();assert_eq!(apply(&f.temp.database,&f.line,1,&f.nonce,&f.owner,&f.scope,&proof),Err(SCOPE));
}
#[test]
fn unknown_and_pending_direct_envelopes_are_not_swept_by_batch_ack_or_before_base() {
    let mut f=Fixture::new();let unknown=f.step(&[(1,&[0xff,0])]).unwrap().remove(0);
    let direct=super::super::native_events::IncomingMessage{body:b"pending direct text".to_vec(),from:Some(b"fixture-original-sender".to_vec()),time:None};f.context.commit_fixture(&f.temp.database,5000,&direct).unwrap();
    for sequence in 2..=150 {f.step(&[(sequence,b"classified batch text")]).unwrap();}f.step(&[]).unwrap();
    assert!(f.context.commit_batch(&f.temp.database,&unknown).is_ok());assert_eq!(f.count("app_voice_inbox"),130);assert_eq!(f.count("sms_messages"),150);
    assert!(!f.temp.database.with_conn(|conn|retired(conn,&f.line,1,&f.nonce,9,1)).unwrap());
}
#[test]
fn held_low_pending_keeps_sparse_tombstones_after_native_base_moves_then_releases_prefix() {
    let mut f=Fixture::new();let first=f.feed(&[(1,b"held original")],true).into_iter().find_map(|event|
        if let OfflineEvent::Message(message)=event{Some(message)}else{None}).unwrap();
    f.context.commit_batch(&f.temp.database,&first).unwrap();
    let second=f.step(&[(2,b"already ACKed original")]).unwrap().remove(0);
    for sequence in 3..=140 {f.step(&[(sequence,b"later batch text")]).unwrap();}
    f.step(&[(8192,b"forward ring jump")]).unwrap();f.step(&[]).unwrap();
    let through:i32=f.temp.database.with_conn(|conn|conn.query_row("SELECT through_sequence FROM app_voice_batch_replay_fences",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(through,0);
    assert_eq!(f.context.commit_batch(&f.temp.database,&second).err(),Some("app_account_inbox_batch_retired"));
    assert!(f.context.commit_batch(&f.temp.database,&first).is_ok(),"pending low original survives the native window movement");
    assert!(f.engine.complete_message(first.ticket(),true).unwrap());
    f.temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_inbox SET reply_state='batch_queued'
        WHERE line_id=?1 AND run_nonce=?2 AND source_kind='batch' AND sequence=1",params![f.line,f.nonce]).map_err(|_|STORAGE)).unwrap();
    f.step(&[]).unwrap();let through:i32=f.temp.database.with_conn(|conn|conn.query_row("SELECT through_sequence FROM app_voice_batch_replay_fences",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(through,6175);
    assert_eq!(f.count("sms_messages"),141);assert!(f.count("app_voice_batch_replay_ids")<3);
    assert_eq!(f.context.commit_batch(&f.temp.database,&second).err(),Some("app_account_inbox_batch_retired"));
}
#[test]
fn sparse_retention_pressure_fails_without_evicting_any_original_tombstone() {
    let mut f=Fixture::new();f.step(&[(1,b"original")]).unwrap();
    f.temp.database.with_conn(|conn|{
        let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
        for sequence in 4000..6048 {tx.execute("INSERT INTO app_voice_batch_replay_ids VALUES(?1,1,?2,9,?3)",params![f.line,f.nonce,sequence]).map_err(|_|STORAGE)?;}
        tx.commit().map_err(|_|STORAGE)
    }).unwrap();
    let proof=f.proof();assert_eq!(apply(&f.temp.database,&f.line,1,&f.nonce,&f.owner,&f.scope,&proof),Err("app_account_batch_retention_full"));
    assert_eq!(f.count("app_voice_batch_replay_ids"),2048);assert_eq!(f.count("app_voice_batch_replay_fences"),0);assert_eq!(f.count("sms_messages"),1);assert_eq!(f.count("app_voice_inbox"),1);
}
