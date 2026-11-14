//! Actions-only account proof: default call routing does not replace media setup.
use super::*;
#[tokio::test]
async fn native_default_control_without_media_never_publishes_answer_ready_or_fake_call_history(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();
    let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());
    let mut config=account_config(&cert.root,port,false);config.profile.native_call_control=true;assert!(config.call_media.is_none());
    let created=manager.create(config,100).await.unwrap();assert!(!created.incoming_call_ready);let line=created.account.line_id;
    let(done,checked)=tokio::sync::oneshot::channel();let serving=tokio::spawn(async move{
        tokio::time::timeout(Duration::from_secs(12),async move{
            let mut peer=CommonPeer::open(listener,server).await;let packet=peer.next().await.unwrap();let login=decode_request(&packet[1..]).unwrap();
            peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");
            let mut args=Writer::versioned().unwrap();args.u64(0x1_2345_6789).unwrap();args.bytes(b"native-peer").unwrap();args.properties(&[]).unwrap();
            peer.send(&[direct(91,n::rpc_calls::INVITED_METHOD.as_bytes(),args.finish().expose_to_transport(),b"CallClientEx",None)]).await;
            assert_eq!(next_reply_for_selector(&mut peer,91,b"fixture-original-selector").await,[0,0,0,0,0,0]);
            done.send(()).unwrap();finish_account(&mut peer).await;
        }).await.unwrap();
    });
    manager.start(&line,1,100).await.unwrap();tokio::time::timeout(Duration::from_secs(6),checked).await.unwrap().unwrap();
    let view=manager.get(&line,100).await.unwrap();assert_eq!(view.phase,"hosted");assert!(view.incoming_message_ready);
    assert!(!view.incoming_call_ready);assert!(!view.outbound_authorized);assert!(view.startup_failure.is_none());
    assert_eq!(temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM call_history",[],|row|row.get::<_,i64>(0)).map_err(|_|"test_query")).unwrap(),0);
    assert!(!manager.stop(&line,1,100).await.unwrap().io_cleanup_required);serving.await.unwrap();
    assert_eq!(manager.get(&line,100).await.unwrap().phase,"stopped");
}
