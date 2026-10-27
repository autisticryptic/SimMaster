//! Actions-only original TLS/ARC/manager reception beyond both old limits.
//! No reconnect/new nonce, cached callback or local SDK/service execution.
use super::*;
use super::super::super::{long_reception_test_support as wire, rpc_envelope::decode_reply, transport_packets::CallKind};
use std::future::Future;
async fn bounded<F:Future>(future:F)->F::Output {tokio::time::timeout(Duration::from_secs(45),future).await.expect("bounded same-run long direct reception")}
fn smgw(root:&str)->super::super::super::account_config::AccountConfig {let mut input=config(root);input.profile.incoming.as_mut().unwrap().receiver_kind=b"Sip.Request".to_vec();input}
fn count(db:&Database,table:&str)->i64 {db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap()}
async fn send(peer:&mut super::super::super::runtime_backend::tests::Peer,id:i32,body:&[u8]) {
    let bytes=wire::wire(id,&id.to_be_bytes(),body);peer.send_call(CallKind::Request,&bytes).await;
}
async fn finish(mut peer:super::super::super::runtime_backend::tests::Peer) {
    peer.swap(Some(STATUS_UNREGISTER),true).await;let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method,super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
}
#[tokio::test]
async fn six_hundred_direct_sms_on_one_original_run_preserve_history_and_bound_private_envelopes() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let(mut peer,connector,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    let observed=temp.database.clone();let id=line.clone();let(done,completed)=oneshot::channel();let(stop,stopped)=oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        for sequence in 1i32..=600 {
            let rpc=100+sequence;let body=format!("fixture sustained SMS {sequence}");send(&mut peer,rpc,body.as_bytes()).await;
            let packet=next_non_poll(&mut peer).await;assert_eq!(packet.kind,CallKind::Reply);let reply=decode_reply(packet.payload.expose_to_transport()).unwrap();assert_eq!(reply.call_id,rpc);assert_eq!(reply.result_stream,&[0,1,0,0,0,0]);
            let total:i64=observed.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM sms_messages WHERE line_id=?1",[&id],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap();
            assert_eq!(total,i64::from(sequence),"each original receipt follows the durable SMS commit");
        }
        // This original RPC identity was compacted from raw storage, not from
        // shared SMS. Repeating it must not dispatch or acknowledge another SMS.
        let duplicate=wire::wire(101,&101i32.to_be_bytes(),b"fixture sustained SMS 1");peer.send_call(CallKind::Request,&duplicate).await;
        assert!(tokio::time::timeout(Duration::from_millis(80),next_non_poll(&mut peer)).await.is_err());
        done.send(()).unwrap();stopped.await.unwrap();finish(peer).await;
    }).await});
    bounded(manager.start(&line,1,100)).await.unwrap();let slot=lock(&manager.slots).get(&line).unwrap().clone();let run=lock(&slot.run).clone().unwrap();let original_nonce=run.inbox_nonce.clone();
    bounded(completed).await.unwrap();bounded(async{loop{
        let through:i32=temp.database.with_conn(|conn|conn.query_row("SELECT through_sequence FROM app_voice_direct_replay_fences WHERE line_id=?1",[&line],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap();
        if through==700{break;}tokio::task::yield_now().await;
    }}).await;
    assert_eq!(count(&temp.database,"sms_messages"),600);assert_eq!(count(&temp.database,"app_voice_sms_projection"),600);assert_eq!(count(&temp.database,"app_voice_inbox"),128);
    assert_eq!(manager.messages(&line,128).unwrap().len(),128);assert_eq!(manager.inbox(&line,128).unwrap().len(),128);
    let state=manager.get(&line,100).await.unwrap();assert!(state.incoming_message_ready);assert!(!state.incoming_call_ready&&!state.outbound_authorized);
    assert_eq!(lock(&slot.run).as_ref().unwrap().inbox_nonce,original_nonce);assert_eq!(factory.build_count(),1);assert_eq!(connector.connection_calls(),1);
    assert!(temp.database.with_conn(|conn|account_runs::require_live(conn,&line,1,&original_nonce)).is_ok());
    let records=temp.database.get_sms_messages_for_channel(700,0,None,Some(&line)).unwrap();assert_eq!(records.len(),600);
    assert!(records.iter().all(|row|row.transport=="app_voice"&&row.pdu.is_none()&&row.direction=="incoming"));
    stop.send(()).unwrap();assert!(!bounded(manager.stop(&line,1,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"app_voice_direct_replay_fences"),0);assert_eq!(count(&temp.database,"sms_messages"),600);
    assert_eq!(factory.build_count(),1);assert!(!temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&line)).unwrap());
}
#[tokio::test]
async fn submitted_direct_reply_gc_failure_retires_original_run_without_replaying_ack_or_sms() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let(mut peer,connector,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_live_direct_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|"test_trigger_failed")).unwrap();
    let serving=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
        for id in 1i32..=129 {
            send(&mut peer,id,b"fixture committed before reply").await;let packet=next_non_poll(&mut peer).await;assert_eq!(packet.kind,CallKind::Reply);assert_eq!(decode_reply(packet.payload.expose_to_transport()).unwrap().call_id,id);
        }
        while let Some(packet)=peer.receive().await{assert_ne!(packet.kind,CallKind::Reply,"already submitted ACK may not be replayed");}
    }).await});
    bounded(manager.start(&line,1,100)).await.unwrap();let state=bounded(async{loop{let view=manager.get(&line,100).await.unwrap();if view.last_error==Some("app_account_inbox_reply_status_unrecorded"){break view;}tokio::task::yield_now().await;}}).await;
    assert!(state.io_cleanup_required);assert!(!state.incoming_message_ready&&!state.outbound_authorized);
    bounded(serving).await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert_eq!(count(&temp.database,"sms_messages"),129);assert_eq!(count(&temp.database,"app_voice_inbox"),129);
    assert!(temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&line)).unwrap());
    temp.database.with_conn(|conn|conn.execute_batch("DROP TRIGGER fail_live_direct_gc").map_err(|_|"test_trigger_failed")).unwrap();
    assert!(!bounded(manager.stop(&line,1,100)).await.unwrap().io_cleanup_required);assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"sms_messages"),129);
    assert_eq!(connector.connection_calls(),1);assert_eq!(factory.build_count(),1);assert_eq!(count(&temp.database,"app_voice_direct_replay_fences"),0);
}
