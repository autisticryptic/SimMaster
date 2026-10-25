// Actions-only real native TLS/Common -> Smgw text -> shared SMS transaction.
use super::*;
use super::super::super::{account_config::AccountConfig,rpc_envelope::decode_reply,runtime_backend::tests::swap_result,
    smgw_text::SMGW_RECEIVER,transport_packets::CallKind};
fn smgw_config(root:&str)->AccountConfig {
    let mut input=config(root);input.profile.incoming.as_mut().unwrap().receiver_kind=SMGW_RECEIVER.to_vec();input
}
async fn finish(mut peer:super::super::super::runtime_backend::tests::Peer) {
    peer.swap(Some(STATUS_UNREGISTER),true).await;let message=next_non_poll(&mut peer).await;
    let request=decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method,super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
}
#[tokio::test]
async fn original_direct_smgw_text_sms_and_both_events_commit_before_native_reply() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(smgw_config(&cert.root),100).await.unwrap().account.line_id;
    let observed=temp.database.clone();let id=line.clone();let (seen,observed_reply)=oneshot::channel();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        peer.incoming_with_fields(91,SMGW_RECEIVER,"fixture 短信 🥘\nline2".as_bytes(),Some(b"fixture-sender"),Some(b"1700000000123")).await;
        let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);assert_eq!(decode_reply(reply.payload.expose_to_transport()).unwrap().call_id,91);
        let rows=observed.get_sms_messages_for_channel(50,0,None,Some(&id)).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].content,"fixture 短信 🥘\nline2");assert_eq!(rows[0].transport,"app_voice");assert!(rows[0].pdu.is_none());
        let events=observed.get_recent_app_events(Some(&id),10).unwrap();assert_eq!(events.len(),2);assert_eq!(events[1].event_type,"sms.received");
        seen.send(()).unwrap();finish(peer).await;
    });
    manager.start(&line,1,100).await.unwrap();tokio::time::timeout(Duration::from_secs(4),observed_reply).await.unwrap().unwrap();
    let rows=manager.messages(&line,50).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].sent_at_unix_millis,Some(1_700_000_000_123));assert_eq!(rows[0].timestamp_source,"sender");
    let view=manager.get(&line,100).await.unwrap();assert!(view.incoming_message_ready);assert!(!view.incoming_call_ready&&!view.outbound_authorized);assert_eq!(view.provider_availability,"available");
    assert_eq!(manager.inbox(&line,50).unwrap()[0].classification,"smgw_text");
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn original_smgw_sms_event_failure_rolls_back_and_never_acks_raw_only_success() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(smgw_config(&cert.root),100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_promoted_event BEFORE INSERT ON app_events WHEN NEW.event_type='sms.received'
        BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|"test_trigger_failed")).unwrap();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        peer.incoming_with_fields(91,SMGW_RECEIVER,b"fixture-text",Some(b"fixture-sender"),None).await;
        while let Some(message)=peer.receive().await {assert_ne!(message.kind,CallKind::Reply);}
    });
    manager.start(&line,1,100).await.unwrap();let state=phase(&manager,&line,"disconnected").await;assert_eq!(state.last_error,Some("app_account_sms_storage_failed"));
    assert!(!state.incoming_message_ready&&!state.incoming_call_ready&&!state.outbound_authorized);
    assert_ne!(manager.messaging_availability(&line),crate::connectivity::app_voice::registry::ProviderAvailability::Available);
    assert!(manager.messages(&line,50).unwrap().is_empty());assert!(manager.inbox(&line,50).unwrap().is_empty());assert!(temp.database.get_recent_app_events(Some(&line),10).unwrap().is_empty());
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn readable_other_receiver_payload_remains_generic_despite_text_sender_and_time() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;let (seen,received)=oneshot::channel();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        peer.incoming_with_fields(91,b"fixture.kind",b"valid-text",Some(b"fixture-sender"),Some(b"1700000000123")).await;
        let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);seen.send(()).unwrap();finish(peer).await;
    });
    manager.start(&line,1,100).await.unwrap();tokio::time::timeout(Duration::from_secs(4),received).await.unwrap().unwrap();
    assert!(manager.messages(&line,50).unwrap().is_empty());assert_eq!(manager.inbox(&line,50).unwrap()[0].classification,"unclassified");
    assert!(temp.database.get_sms_messages_for_channel(50,0,None,Some(&line)).unwrap().is_empty());assert_eq!(temp.database.get_recent_app_events(Some(&line),10).unwrap().len(),1);
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn native_batch_smgw_projection_precedes_queued_consumer_result_without_invented_sent_time() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(smgw_config(&cert.root),100).await.unwrap().account.line_id;let observed=temp.database.clone();let id=line.clone();let (done,saw)=oneshot::channel();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=super::super::super::rpc_batch::read_payload(&mut args).unwrap();
        let mut incoming=Writer::versioned().unwrap();incoming.bytes(SMGW_RECEIVER).unwrap();incoming.properties(&[super::super::super::rpc_stream::RpcProperty{key:b"Sip.Message.From",value:b"fixture-sender"}]).unwrap();incoming.bytes(b"literal 00FF not PDU").unwrap();
        let mut response=Writer::new();response.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{response.u32(word).unwrap();}
        response.u32(1).unwrap();response.i32(1).unwrap();response.bytes(super::super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();response.bytes(incoming.finish().expose_to_transport()).unwrap();
        response.properties(&[super::super::super::rpc_stream::RpcProperty{key:b"id",value:b"#fixture-client"},super::super::super::rpc_stream::RpcProperty{key:b"domain",value:b"fixture-domain"},super::super::super::rpc_stream::RpcProperty{key:b"__magic",value:&[0,0xff]}]).unwrap();
        response.u32(0).unwrap();response.properties(&[]).unwrap();response.bytes(b"").unwrap();peer.reply(request.call_id,response.finish().expose_to_transport()).await;
        loop {
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=super::super::super::rpc_batch::read_payload(&mut args).unwrap();let completed=payload.results.get(&1);
            if let Some(result)=completed {assert_eq!(result.result_stream,&[0,1,0,0,0,0]);let sms=observed.get_sms_messages_for_channel(10,0,None,Some(&id)).unwrap();assert_eq!(sms.len(),1);assert_eq!(sms[0].content,"literal 00FF not PDU");assert_eq!(observed.get_recent_app_events(Some(&id),10).unwrap().len(),2);}
            peer.reply(request.call_id,&swap_result(None,true,payload.fields)).await;if completed.is_some(){break;}
        }
        done.send(()).unwrap();finish(peer).await;
    });
    manager.start(&line,1,100).await.unwrap();tokio::time::timeout(Duration::from_secs(4),saw).await.unwrap().unwrap();
    let messages=manager.messages(&line,50).unwrap();assert_eq!(messages[0].sent_at_unix_millis,None);assert_eq!(messages[0].timestamp_source,"received");assert_eq!(manager.inbox(&line,50).unwrap()[0].reply_state,"batch_queued");
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
