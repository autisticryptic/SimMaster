//! Actions-only native Router/TLS/Common -> actual account IM persistence and
//! original Message/MoreMessage replies, never a real SDK/device/Provider call.
use super::*;
use n::im_delivery::NativeImConfig;
use std::future::Future;
async fn bounded<F:Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(12),future).await.expect("original IM account fixture deadline")}
fn config(root:&str,port:u16,timeout:u64)->AccountConfig{let mut cfg=account_config(root,port,false);cfg.profile.native_receivers.as_mut().unwrap().im_messages=Some(NativeImConfig{notification_timeout_ms:timeout});cfg}
fn hint(rpc:i32,id:u64,index:i64)->Vec<u8>{let idx=index.to_string();let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();w.properties(&[RpcProperty{key:b"Notify.Command",value:b"Message"},RpcProperty{key:b"Notify.Box",value:b"P2P/1_2"},RpcProperty{key:b"Notify.MsgIdx",value:idx.as_bytes()}]).unwrap();let args=w.finish();let magic=rpc.to_be_bytes();let raw=encode_request(rpc,&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"},RpcProperty{key:b"__magic",value:&magic}],n::rpc_client_ex::NOTIFY_CALLS_METHOD.as_bytes(),args.expose_to_transport()).unwrap();let mut p=vec![0];p.extend_from_slice(raw.expose_to_transport());p}
fn message(rpc:i32,index:i64,valid:bool)->Vec<u8>{
    let mut raw=Writer::new();raw.i64(index).unwrap();raw.i64(1234567890123).unwrap();raw.bytes(b"fixture-private-uid").unwrap();raw.bytes(b"Text").unwrap();raw.bytes(if valid{br#"{"text":"original IM text","userData":"private-not-public"}"#}else{b"malformed-json"}).unwrap();raw.properties(&[RpcProperty{key:b"../never-written",value:b"attachment-bytes"}]).unwrap();raw.properties(&[]).unwrap();
    let mut args=Writer::versioned().unwrap();args.bytes(n::im_data::MORE_MESSAGE_KIND).unwrap();args.properties(&[RpcProperty{key:b"Box",value:b"P2P/1_2"}]).unwrap();args.bytes(raw.finish().expose_to_transport()).unwrap();direct(rpc,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),args.finish().expose_to_transport(),b"MessageReceiver",None)
}
async fn next_reply(peer:&mut CommonPeer,id:i32,accepted:bool){
    loop{let p=peer.next().await.unwrap();if p[0]==1{let r=decode_reply(&p[1..]).unwrap();assert_eq!(r.call_id,id);assert_eq!(r.result_code,0);assert_eq!(r.result_stream,if accepted{[0,1,0,0,0,0]}else{[0;6]});return;}
        let request=decode_request(&p[1..]).unwrap();assert_eq!(request.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(),"IM consumes original inbound data, not fabricated reads/outbound sends");let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let payload=n::rpc_batch::read_payload(&mut r).unwrap();assert!(payload.results.is_empty());peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;
    }
}
fn count(db:&crate::platform::db::Database)->i64{db.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_im_messages",[],|row|row.get(0)).map_err(|_|"fixture")).unwrap()}
async fn scenario(mode:u8){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,if mode>=2{100}else{10000}),100).await.unwrap().account;let line=account.line_id.clone();let db=temp.database.clone();
    let(go,wait)=tokio::sync::oneshot::channel();let(ready,observed)=tokio::sync::oneshot::channel();let(replay,send_replay)=tokio::sync::oneshot::channel();let(done,finished)=tokio::sync::oneshot::channel();let(stop,stopping)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,status)=hosting_call(&mut peer).await;assert_eq!(status,b"4");wait.await.unwrap();
        if mode==1{peer.send(&[message(91,0x1_2345_6789,true)]).await;next_reply(&mut peer,91,true).await;peer.send(&[hint(92,0xf123_4567_89ab_cdef,0x1_2345_6789)]).await;next_reply(&mut peer,92,true).await;}
        else{peer.send(&[hint(91,0xf123_4567_89ab_cdef,0x1_2345_6789)]).await;if mode!=2{peer.send(&[message(92,0x1_2345_6789,mode!=3)]).await;next_reply(&mut peer,92,mode!=3).await;}next_reply(&mut peer,91,mode<2).await;}
        assert_eq!(count(&db),i64::from(mode<2));ready.send(()).unwrap();send_replay.await.unwrap();
        if mode<2{peer.send(&[message(93,0x1_2345_6789,true),hint(94,0xf123_4567_89ab_cdef,0x1_2345_6789)]).await;next_reply(&mut peer,93,true).await;next_reply(&mut peer,94,true).await;assert_eq!(count(&db),1);}
        done.send(()).unwrap();stopping.await.unwrap();finish_account(&mut peer).await;
    }).await});
    bounded(manager.start(&line,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&line,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();
    let rows=manager.im_messages(&line,16).unwrap();assert_eq!(rows.len(),usize::from(mode<2));if let Some(row)=rows.first(){assert_eq!(row.text,"original IM text");assert_eq!(row.message_index,"4886718345");assert_eq!(row.attachment_count,1);let json=serde_json::to_string(&rows).unwrap();for private in ["private-not-public","fixture-private-uid","attachment-bytes","never-written","identity_key","box_id"]{assert!(!json.contains(private));}assert!(manager.delete_im_message(&line,row.id).unwrap());}
    replay.send(()).unwrap();bounded(finished).await.unwrap();assert!(manager.im_messages(&line,16).unwrap().is_empty());assert!(manager.messages(&line,16).unwrap().is_empty());assert!(temp.database.get_call_history_for_line(&line,16,0).unwrap().is_empty());let view=manager.get(&line,100).await.unwrap();assert_eq!(view.phase,"hosted");assert!(!view.incoming_call_ready&&!view.outbound_authorized);
    let events=temp.database.get_recent_app_events(Some(&line),16).unwrap();assert_eq!(events.iter().filter(|event|event.event_type=="im.received").count(),usize::from(mode<2));stop.send(()).unwrap();let stopped=bounded(manager.stop(&line,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert!(!temp.database.with_conn(|conn|n::account_runs::ownership_pending(conn,&line)).unwrap());
}
#[tokio::test] async fn im_account_current_message_hint_waits_for_durable_text_then_original_replies_and_delete_does_not_resurrect(){Box::pin(scenario(0)).await;}
#[tokio::test] async fn im_account_moremessage_before_notification_uses_persistent_original_identity_without_duplicate_events(){Box::pin(scenario(1)).await;}
#[tokio::test] async fn im_account_missing_notified_payload_expires_negative_not_sms_or_fake_readiness(){Box::pin(scenario(2)).await;}
#[tokio::test] async fn im_account_malformed_text_is_not_committed_and_never_completes_notification_positive(){Box::pin(scenario(3)).await;}
#[tokio::test]
async fn im_account_early_native_messages_wait_for_original_hosting_ack_and_fixed_tickets(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,3000),100).await.unwrap().account;let db=temp.database.clone();let(ready,observed)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{bounded(async move{
            let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result()),hint(91,99,7),message(92,7,true)]).await;
            loop{let p=peer.next().await.unwrap();assert_eq!(p[0],0,"no unconfirmed IM ACK");let request=decode_request(&p[1..]).unwrap();assert_eq!(request.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(request.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();let seq=payload.requests.keys().copied().next();if seq.is_some(){tokio::time::sleep(Duration::from_millis(30)).await;assert_eq!(count(&db),0);}peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(seq,true,payload.fields))]).await;if seq.is_some(){break;}}
            next_reply(&mut peer,92,true).await;next_reply(&mut peer,91,true).await;assert_eq!(count(&db),1);ready.send(()).unwrap();finish_account(&mut peer).await;
        }).await});
        bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(observed).await.unwrap();assert_eq!(manager.im_messages(&account.line_id,16).unwrap().len(),1);bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(serving).await.unwrap();
    }).await;
}
#[tokio::test]
async fn im_account_authenticated_batch_persists_both_original_results_before_transport_epoch_ack(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,3000),100).await.unwrap().account;let db=temp.database.clone();let(go,wait)=tokio::sync::oneshot::channel();let(done,observed)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{bounded(async move{
            let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();
            let p=peer.next().await.unwrap();let swap=decode_request(&p[1..]).unwrap();assert_eq!(swap.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(swap.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();
            let hint_packet=hint(91,99,7);let message_packet=message(92,7,true);let hint=decode_request(&hint_packet[1..]).unwrap();let decoded_message=decode_request(&message_packet[1..]).unwrap();let mut w=Writer::new();w.boolean(true).unwrap();for value in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{w.u32(value).unwrap();}w.u32(2).unwrap();
            for(seq,request)in[(1,&hint),(2,&decoded_message)]{w.i32(seq).unwrap();w.bytes(request.method).unwrap();w.bytes(request.arguments).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"}]).unwrap();}w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();peer.send(&[reply(swap.call_id,w.finish().expose_to_transport())]).await;
            let mut seen=std::collections::BTreeSet::new();while seen.len()<2{let p=peer.next().await.unwrap();assert_eq!(p[0],0);let swap=decode_request(&p[1..]).unwrap();assert_eq!(swap.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(swap.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let payload=n::rpc_batch::read_payload(&mut r).unwrap();for(seq,result)in &payload.results{assert!([1,2].contains(seq));assert_eq!(result.result_stream,[0,1,0,0,0,0]);assert_eq!(count(&db),1);seen.insert(*seq);}peer.send(&[reply(swap.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;}
            // A peer write is not the actor's processed epoch ACK. A fresh
            // original DIRECT response on this same stream is the barrier:
            // it follows the ACK in receive order and still commits no new row.
            peer.send(&[message(95,7,true)]).await;next_reply(&mut peer,95,true).await;assert_eq!(count(&db),1);
            done.send(()).unwrap();finish_account(&mut peer).await;
        }).await});
        bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();assert_eq!(manager.im_messages(&account.line_id,16).unwrap().len(),1);assert!(manager.messages(&account.line_id,16).unwrap().is_empty());bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(serving).await.unwrap();
    }).await;
}
async fn duplicate_after_deadline(consumed_before:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,500),100).await.unwrap().account;let(go,wait)=tokio::sync::oneshot::channel();let(done,observed)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();peer.send(&[hint(91,99,7)]).await;
        if consumed_before{peer.send(&[message(92,7,true)]).await;next_reply(&mut peer,92,true).await;}next_reply(&mut peer,91,consumed_before).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        if !consumed_before{peer.send(&[message(92,7,true)]).await;next_reply(&mut peer,92,true).await;}
        peer.send(&[hint(93,99,7)]).await;next_reply(&mut peer,93,consumed_before).await;done.send(()).unwrap();finish_account(&mut peer).await;
    }).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();assert_eq!(manager.im_messages(&account.line_id,16).unwrap().len(),1);assert_eq!(temp.database.get_recent_app_events(Some(&account.line_id),16).unwrap().iter().filter(|event|event.event_type=="im.received").count(),1);bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(serving).await.unwrap();
}
#[tokio::test] async fn im_account_duplicate_after_consumed_window_replays_proven_result_on_its_own_original_ticket(){Box::pin(duplicate_after_deadline(true)).await;}
#[tokio::test] async fn im_account_expired_unconsumed_notification_is_not_revived_by_later_message_or_new_rpc(){Box::pin(duplicate_after_deadline(false)).await;}
#[tokio::test]
async fn im_account_stop_drops_unconsumed_notification_without_positive_reply_and_joins_original_sdk(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,30000),100).await.unwrap().account;let(go,wait)=tokio::sync::oneshot::channel();let(sent,received)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();peer.send(&[hint(91,99,7)]).await;sent.send(()).unwrap();finish_account(&mut peer).await;}).await});
        bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(received).await.unwrap();tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(manager.im_messages(&account.line_id,16).unwrap().is_empty());let stopped=bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert!(!temp.database.with_conn(|conn|n::account_runs::ownership_pending(conn,&account.line_id)).unwrap());
    }).await;
}
