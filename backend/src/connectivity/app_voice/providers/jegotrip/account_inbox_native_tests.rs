// Actions-only original native consumer replies after actual SQLite commits.
use super::*;
use super::super::super::{rpc_envelope::decode_reply,runtime_backend::tests::swap_result,transport_packets::CallKind};

#[tokio::test]
async fn generic_direct_message_commits_before_original_reply_without_becoming_sms_ready() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
    let (mut peer,connector,dropped)=peer(&cert,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);
    let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;let observed=temp.database.clone();let id=line.clone();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;peer.incoming(91).await;
        let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);
        assert_eq!(decode_reply(reply.payload.expose_to_transport()).unwrap().call_id,91);
        let rows:i64=observed.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_inbox WHERE line_id=?1",[&id],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap();assert_eq!(rows,1);
        let events=observed.get_recent_app_events(Some(&id),10).unwrap();assert_eq!(events.len(),1);assert_eq!(events[0].event_type,"app_voice.inbox_stored");
        peer.swap(Some(STATUS_UNREGISTER),true).await;let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();
        peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
    });
    manager.start(&line,1,100).await.unwrap();phase(&manager,&line,"hosted").await;
    let inbox=tokio::time::timeout(Duration::from_secs(4),async {loop{let inbox=manager.inbox(&line,50).unwrap();if inbox.first().is_some_and(|row|row.reply_state=="direct_submitted"){break inbox;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
    assert_eq!(inbox.len(),1);assert_eq!(inbox[0].classification,"unclassified");let state=manager.get(&line,100).await.unwrap();
    assert!(!state.incoming_message_ready&&!state.incoming_call_ready&&!state.outbound_authorized);
    let sms:i64=temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM sms_messages",[],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap();assert_eq!(sms,0);
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert_eq!(connector.connection_calls(),1);
}

#[tokio::test]
async fn failed_atomic_event_has_no_original_consumer_reply_or_partial_sms() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_inbox_event BEFORE INSERT ON app_events WHEN NEW.event_type='app_voice.inbox_stored'
        BEGIN SELECT RAISE(ABORT,'fixture-private-do-not-echo');END;").map_err(|_|"test_trigger_failed")).unwrap();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;peer.incoming(91).await;
        while let Some(message)=peer.receive().await {assert_ne!(message.kind,CallKind::Reply);let request=decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method,super::super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());}
    });
    manager.start(&line,1,100).await.unwrap();let state=phase(&manager,&line,"disconnected").await;assert_eq!(state.last_error,Some("app_account_inbox_storage_failed"));
    assert!(manager.inbox(&line,50).unwrap().is_empty());assert!(temp.database.get_recent_app_events(Some(&line),10).unwrap().is_empty());
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn reply_marker_failure_preserves_body_and_does_not_replay_consumed_ticket() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;let observed=temp.database.clone();let id=line.clone();
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_inbox_marker BEFORE UPDATE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-marker');END;").map_err(|_|"test_trigger_failed")).unwrap();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;peer.incoming(91).await;
        let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);assert_eq!(decode_reply(reply.payload.expose_to_transport()).unwrap().call_id,91);
        let rows=observed.with_conn(|conn|super::super::super::account_inbox::metadata(conn,&id,50)).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].classification,"unclassified");
        while let Some(message)=peer.receive().await {assert_ne!(message.kind,CallKind::Reply);}
    });
    manager.start(&line,1,100).await.unwrap();let state=phase(&manager,&line,"disconnected").await;
    assert_eq!(state.last_error,Some("app_account_inbox_reply_status_unrecorded"));let rows=manager.inbox(&line,50).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].reply_state,"pending");
    assert_eq!(temp.database.get_recent_app_events(Some(&line),10).unwrap().len(),1);
    manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn batch_response_is_queued_only_after_private_data_and_event_commit() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;let observed=temp.database.clone();let id=line.clone();
    let (completed,saw_completion)=oneshot::channel();
    let serving=tokio::spawn(async move {
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();
        let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=super::super::super::rpc_batch::read_payload(&mut args).unwrap();
        let mut incoming=Writer::versioned().unwrap();incoming.bytes(b"fixture.kind").unwrap();incoming.properties(&[]).unwrap();incoming.bytes(&[0xff,0,1]).unwrap();
        let mut response=Writer::new();response.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{response.u32(word).unwrap();}
        response.u32(1).unwrap();response.i32(1).unwrap();response.bytes(super::super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();response.bytes(incoming.finish().expose_to_transport()).unwrap();
        response.properties(&[super::super::super::rpc_stream::RpcProperty{key:b"id",value:b"#fixture-client"},super::super::super::rpc_stream::RpcProperty{key:b"domain",value:b"fixture-domain"},super::super::super::rpc_stream::RpcProperty{key:b"__magic",value:&[0,0xff]}]).unwrap();
        response.u32(0).unwrap();response.properties(&[]).unwrap();response.bytes(b"").unwrap();peer.reply(request.call_id,response.finish().expose_to_transport()).await;
        loop {
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();let mut args=Reader::versioned(request.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();let payload=super::super::super::rpc_batch::read_payload(&mut args).unwrap();
            let complete=payload.results.get(&1);if let Some(result)=complete {assert_eq!(result.result_stream,&[0,1,0,0,0,0]);
                assert_eq!(observed.with_conn(|conn|super::super::super::account_inbox::metadata(conn,&id,50)).unwrap().len(),1);assert_eq!(observed.get_recent_app_events(Some(&id),10).unwrap().len(),1);}
            peer.reply(request.call_id,&swap_result(None,true,payload.fields)).await;if complete.is_some(){break;}
        }
        completed.send(()).unwrap();peer.swap(Some(STATUS_UNREGISTER),true).await;let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
    });
    manager.start(&line,1,100).await.unwrap();tokio::time::timeout(Duration::from_secs(4),saw_completion).await.unwrap().unwrap();
    let rows=manager.inbox(&line,50).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].reply_state,"batch_queued");assert_eq!(rows[0].classification,"unclassified");
    assert!(!manager.get(&line,100).await.unwrap().incoming_message_ready);manager.stop(&line,1,100).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
