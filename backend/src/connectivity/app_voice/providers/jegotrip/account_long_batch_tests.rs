//! Actions-only actual original TLS/ARC/manager batch result WRITE + ACK.
//! Same connector/run nonce; no local execution, reconnect or fake receipt.
use super::*;
use super::super::super::{runtime_backend::tests::Peer,rpc_batch,rpc_incoming::ONLINE_MESSAGE_METHOD,
    rpc_stream::RpcProperty,smgw_text::SMGW_RECEIVER,transport_packets::CallKind};
use std::{collections::BTreeSet,future::Future};
async fn bounded<F:Future>(future:F)->F::Output {tokio::time::timeout(Duration::from_secs(90),future).await.expect("bounded same-run batch reception")}
fn smgw(root:&str)->super::super::super::account_config::AccountConfig {let mut imported=config(root);imported.profile.incoming.as_mut().unwrap().receiver_kind=SMGW_RECEIVER.to_vec();imported}
fn count(db:&Database,table:&str)->i64 {db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap()}
fn response(words:[u32;6],peer_next:u32,requests:&[(i32,Vec<u8>)],nested:Option<i32>)->Vec<u8> {
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,peer_next,1,words[0],words[1],words[2]]{w.u32(word).unwrap();}
    w.u32(requests.len() as u32).unwrap();
    for(sequence,body) in requests {
        let mut args=Writer::versioned().unwrap();args.bytes(SMGW_RECEIVER).unwrap();args.properties(&[RpcProperty{key:b"Sip.Message.From",value:b"fixture-original-sender"}]).unwrap();args.bytes(body).unwrap();
        w.i32(*sequence).unwrap();w.bytes(ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();w.bytes(args.finish().expose_to_transport()).unwrap();
        w.properties(&[RpcProperty{key:b"id",value:b"#fixture-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:&sequence.to_be_bytes()}]).unwrap();
    }
    w.u32(u32::from(nested.is_some())).unwrap();if let Some(sequence)=nested{w.i32(sequence).unwrap();w.u32(0).unwrap();w.bytes(&[0,1,0,0,0,0]).unwrap();}
    w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
async fn receive(peer:&mut Peer,total:i32,db:&Database,line:&str)->u32 {
    let mut sent=0i32;let mut peer_next=1u32;let mut observed=BTreeSet::new();let mut waiting=BTreeSet::new();
    loop {
        let packet=peer.receive().await.unwrap();assert_eq!(packet.kind,CallKind::Request);let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=rpc_batch::read_payload(&mut args).unwrap();
        assert!(payload.requests.is_empty(),"incoming-only reception creates no new client business request");
        for(&sequence,result) in &payload.results {
            assert!(sequence>0&&sequence<=sent);assert_eq!(result.result_code,0);assert_eq!(result.result_stream,&[0,1,0,0,0,0]);
            assert!(observed.insert(sequence),"already ACKed result must not be transmitted again");waiting.remove(&sequence);
            let content:Option<String>=db.with_conn(|conn|conn.query_row("SELECT content FROM sms_messages WHERE line_id=?1 AND content=?2",
                rusqlite::params![line,format!("fixture batch SMS {sequence}")],|row|row.get(0)).map(Some).map_err(|_|"test_query_failed")).unwrap();
            assert_eq!(content,Some(format!("fixture batch SMS {sequence}")),"original result bytes follow raw/SMS/event commit");
        }
        let mut incoming=Vec::new();
        if waiting.is_empty()&&sent<total {
            peer_next+=1;
            for _ in 0..8 {if sent==total{break;}sent+=1;waiting.insert(sent);incoming.push((sent,format!("fixture batch SMS {sent}").into_bytes()));}
        }
        let finished=sent==total&&waiting.is_empty();
        peer.reply(request.call_id,&response(payload.fields,peer_next,&incoming,None)).await;
        if finished {assert_eq!(observed.len(),total as usize);return peer_next;}
    }
}
async fn finish(mut peer:Peer,peer_next:u32) {
    loop {
        let packet=peer.receive().await.unwrap();let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=rpc_batch::read_payload(&mut args).unwrap();let nested=payload.requests.keys().next().copied();
        if let Some(sequence)=nested {
            let call=&payload.requests[&sequence];let mut reader=Reader::versioned(call.arguments).unwrap();reader.bytes().unwrap();let props=reader.properties().unwrap();
            assert_eq!(props.iter().find(|p|p.key==b"SipFlag").unwrap().value,b"3");
        }
        peer.reply(request.call_id,&response(payload.fields,peer_next,&[],nested)).await;if nested.is_some(){break;}
    }
    let packet=next_non_poll(&mut peer).await;let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
}
#[tokio::test]
async fn six_hundred_batch_sms_on_one_original_run_use_exact_ack_and_durable_sparse_retirement() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let(mut peer,connector,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    let observed=temp.database.clone();let id=line.clone();let(done,completed)=oneshot::channel();let(stop,stopped)=oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        let peer_next=receive(&mut peer,600,&observed,&id).await;
        // Repeated original sequence with a changed body cannot re-enter business
        // delivery after its private envelope was compacted under exact ACK.
        let packet=peer.receive().await.unwrap();let request=decode_request(packet.payload.expose_to_transport()).unwrap();let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=rpc_batch::read_payload(&mut args).unwrap();
        peer.reply(request.call_id,&response(payload.fields,peer_next,&[(1,b"must not resurrect".to_vec())],None)).await;
        done.send(()).unwrap();stopped.await.unwrap();finish(peer,peer_next).await;
    }).await});
    bounded(manager.start(&line,1,100)).await.unwrap();let slot=lock(&manager.slots).get(&line).unwrap().clone();let run=lock(&slot.run).clone().unwrap();let nonce=run.inbox_nonce.clone();
    bounded(completed).await.unwrap();bounded(async{loop{if count(&temp.database,"app_voice_batch_replay_ids")==600&&count(&temp.database,"app_voice_inbox")==128{break;}tokio::task::yield_now().await;}}).await;
    assert_eq!(count(&temp.database,"sms_messages"),600);assert_eq!(count(&temp.database,"app_voice_sms_projection"),600);assert_eq!(count(&temp.database,"app_voice_direct_replay_fences"),0);
    assert_eq!(manager.messages(&line,128).unwrap().len(),128);assert_eq!(manager.inbox(&line,128).unwrap().len(),128);
    let state=manager.get(&line,100).await.unwrap();assert!(state.incoming_message_ready);assert!(!state.incoming_call_ready&&!state.outbound_authorized);
    assert_eq!(lock(&slot.run).as_ref().unwrap().inbox_nonce,nonce);assert_eq!(connector.connection_calls(),1);assert_eq!(factory.build_count(),1);
    stop.send(()).unwrap();assert!(!bounded(manager.stop(&line,1,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"app_voice_batch_replay_ids"),0);assert_eq!(count(&temp.database,"app_voice_batch_replay_fences"),0);assert_eq!(count(&temp.database,"sms_messages"),600);
    assert!(!temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&line)).unwrap());
}
#[tokio::test]
async fn post_ack_batch_gc_failure_retires_original_scope_without_requeuing_acknowledged_results() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let(mut peer,connector,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_live_batch_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|"test_trigger_failed")).unwrap();
    let observed=temp.database.clone();let id=line.clone();let serving=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;receive(&mut peer,129,&observed,&id).await;
        while let Some(packet)=peer.receive().await {
            assert_ne!(packet.kind,CallKind::Reply);let request=decode_request(packet.payload.expose_to_transport()).unwrap();
            let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();
            assert!(rpc_batch::read_payload(&mut args).unwrap().results.is_empty(),"already ACKed result may not replay after failed local GC");
        }
    }).await});
    bounded(manager.start(&line,1,100)).await.unwrap();let state=bounded(async{loop{let view=manager.get(&line,100).await.unwrap();if view.last_error==Some("app_account_batch_retention_storage_failed"){break view;}tokio::task::yield_now().await;}}).await;
    assert!(state.io_cleanup_required);assert!(!state.incoming_message_ready&&!state.outbound_authorized);bounded(serving).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(count(&temp.database,"sms_messages"),129);assert_eq!(count(&temp.database,"app_voice_inbox"),129);assert_eq!(count(&temp.database,"app_voice_batch_replay_ids"),128);
    temp.database.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_live_batch_gc").map_err(|_|"test_trigger_failed")).unwrap();
    assert!(!bounded(manager.stop(&line,1,100)).await.unwrap().io_cleanup_required);assert_eq!(count(&temp.database,"sms_messages"),129);assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"app_voice_batch_replay_ids"),0);
    assert_eq!(connector.connection_calls(),1);assert_eq!(factory.build_count(),1);
}
