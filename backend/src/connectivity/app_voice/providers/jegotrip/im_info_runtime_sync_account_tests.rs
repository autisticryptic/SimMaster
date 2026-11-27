//! Actions-only original Hosted consumer: Info recv, ordering, SQL and joins.
use super::*;
use super::super::info::{BODY,info_packet,no_write_markers,private_view};
// Only this module's stop/late-Info case calls hosting_call directly. Normal
// login_host and finish_account keep their original immediate-result peers.
// A peer write does not prove the actor processed its epoch ACK. Return the
// status-3 result only on a later empty poll from the acknowledged epoch.
async fn hosting_call(peer: &mut CommonPeer) -> (Vec<u8>, Vec<u8>) {
    Box::pin(unhost_after_epoch_ack(peer)).await
}
fn stop_poll(packet: &[u8]) -> (n::rpc_envelope::RpcRequest<'_>, n::rpc_batch::BatchPayload<'_>) {
    assert_eq!(packet.first(), Some(&0));
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
    let mut args = Reader::versioned(request.arguments).unwrap();
    assert_eq!(args.bytes().unwrap(), b"fixture-user");
    assert_eq!(args.bytes().unwrap(), b"fixture-original-selector");
    let batch = n::rpc_batch::read_payload(&mut args).unwrap();
    args.finish().unwrap();
    assert!(batch.results.is_empty(), "no Info completion during stop");
    (request, batch)
}
fn retained_status(original: &n::rpc_envelope::RpcRequest<'_>, next: &n::rpc_envelope::RpcRequest<'_>) -> bool {
    // Only the outer id changes. Equality includes the epoch words and every
    // byte of the nested sequence, route, arguments, and both maps.
    next.call_id > original.call_id && next.method == original.method
        && next.metadata == original.metadata && next.arguments == original.arguments
}
async fn unhost_after_epoch_ack(peer: &mut CommonPeer) -> (Vec<u8>, Vec<u8>) {
    let first = loop {
        let packet = peer.next().await.unwrap();
        let (request, batch) = stop_poll(&packet);
        if !batch.requests.is_empty() { drop((request, batch)); break packet; }
        peer.send(&[reply(request.call_id, &n::runtime_backend::tests::swap_result(None, true, batch.fields))]).await;
    };
    let (original, batch) = stop_poll(&first);
    assert_eq!(batch.requests.len(), 1);
    let (&sequence, nested) = batch.requests.first_key_value().unwrap();
    assert!(sequence > 0 && batch.fields[2] == 1);
    assert_eq!(nested.method, n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
    assert_eq!(nested.metadata.iter().find(|p| p.key == b"id").unwrap().value, n::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes());
    let mut args = Reader::versioned(nested.arguments).unwrap();
    let uri = args.bytes().unwrap().to_vec();
    let flag = args.properties().unwrap().iter().find(|p| p.key == b"SipFlag").unwrap().value.to_vec();
    args.finish().unwrap();
    assert_eq!(flag, b"3");
    peer.send(&[reply(original.call_id, &n::runtime_backend::tests::swap_result(None, true, batch.fields))]).await;
    let mut last = original.call_id;
    loop {
        let packet = peer.next().await.unwrap();
        let (next, poll) = stop_poll(&packet);
        assert!(next.call_id > last && next.metadata == original.metadata);
        last = next.call_id;
        if !poll.requests.is_empty() {
            assert!(retained_status(&original, &next), "new work is not a retained unregister");
            peer.send(&[reply(last, &n::runtime_backend::tests::swap_result(None, true, poll.fields))]).await;
            continue;
        }
        assert_eq!(poll.fields[..3], [batch.fields[0], batch.fields[1], 0]);
        peer.send(&[reply(last, &n::runtime_backend::tests::swap_result(Some(sequence), true, poll.fields))]).await;
        return (uri, flag);
    }
}
#[path="im_info_stop_poll_fixture_tests.rs"]
mod poll_fence;
fn info_config(root:&str,port:u16,timeout:u64)->AccountConfig{let mut c=config(root,port,timeout);c.profile.native_receivers.as_mut().unwrap().im_messages.as_mut().unwrap().receive_info=true;c}
fn mixed_records(rows:&[(i64,&[u8],&[u8])])->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(rows.len()as u32).unwrap();for(index,kind,body)in rows{w.i64(*index).unwrap();w.i64(1234567890123).unwrap();w.bytes(b"fixture-private-uid").unwrap();w.bytes(kind).unwrap();w.bytes(body).unwrap();w.properties(&[RpcProperty{key:b"../never-written",value:b"attachment-bytes"}]).unwrap();w.properties(&[]).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
async fn mixed(mode:u8){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(info_config(&cert.root,port,if mode==2{100}else{3000}),100).await.unwrap().account;let line=account.line_id.clone();let db=temp.database.clone();let server_line=line.clone();let(done,finished)=tokio::sync::oneshot::channel();let(admitted,wait_admission)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;
        if mode==1{let p=group(&mut peer,n::im_status::REFRESH_METHOD).await;let id=decode_request(&p[1..]).unwrap().call_id;peer.send(&[info_packet(91,3,false,BODY),hint(92,99,3)]).await;wait_admission.await.unwrap();assert_eq!(super::super::count(&db),0);peer.send(&[reply(id,&status(0,2,1))]).await;}
        else{drop(wait_admission);refresh(&mut peer,0,if mode==2{2}else{3},1).await;}
        let p=group(&mut peer,n::im_data::RECV_METHOD).await;let(id,start,count)=receive_span(&p);assert_eq!((start,count),(1,if mode==2{2}else{3}));assert_eq!(super::super::count(&db),0);
        let rows=if mode==0{vec![(1,b"Text".as_slice(),br#"{"text":"original IM text"}"#.as_slice()),(2,b"Info.Notice".as_slice(),BODY),(3,b"System.Info.Notice".as_slice(),BODY)]}
        else if mode==1{vec![(1,b"Text".as_slice(),br#"{"text":"original IM text"}"#.as_slice()),(2,b"System.Info.Notice".as_slice(),BODY)]}
        else{vec![(1,b"Text".as_slice(),br#"{"text":"original IM text"}"#.as_slice()),(2,b"Info.Notice".as_slice(),br#"{"content":"bad","OnlineMsg":true}"#.as_slice())]};
        peer.send(&[reply(id,&mixed_records(&rows))]).await;
        if mode==1{next_reply(&mut peer,91,true).await;next_reply(&mut peer,92,true).await;}
        else{peer.send(&[hint(91,99,if mode==2{1}else{3})]).await;next_reply(&mut peer,91,mode!=2).await;}
        assert_eq!(cursor(&db,&server_line),if mode==2{(0,0,2)}else{(0,3,3)});assert_eq!(super::super::count(&db),if mode==2{0}else{3});no_write_markers(&db);done.send(()).unwrap();finish_account(&mut peer).await;
    }).await});
    bounded(manager.start(&line,account.revision,100)).await.unwrap();if mode==1{bounded(async{while manager.im_original_queue_admissions(&line).unwrap()<1{tokio::time::sleep(Duration::from_millis(5)).await;}}).await;admitted.send(()).unwrap();}else{drop(admitted);}
    bounded(finished).await.unwrap();let rows=manager.im_messages(&line,128).unwrap();assert_eq!(rows.len(),if mode==2{0}else{3});if mode!=2{assert_eq!(rows.iter().filter(|r|r.kind==Some("info")).count(),2);assert!(rows.iter().filter(|r|r.kind==Some("info")).all(|r|r.info_type.as_deref()==Some("Notice")));private_view(&serde_json::to_string(&rows).unwrap());}
    let view=manager.get(&line,100).await.unwrap();assert_eq!(view.phase,"hosted");assert!(!view.incoming_call_ready&&!view.outbound_authorized);if mode==2{assert_eq!(view.last_error,Some("app_account_im_message_invalid"));}
    assert!(manager.messages(&line,16).unwrap().is_empty());assert!(temp.database.get_call_history_for_line(&line,16,0).unwrap().is_empty());assert_eq!(temp.database.get_recent_app_events(Some(&line),16).unwrap().iter().filter(|e|e.event_type=="im.received").count(),if mode==2{0}else{3});no_write_markers(&temp.database);
    assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();assert!(!temp.database.with_conn(|c|n::account_runs::ownership_pending(c,&line)).unwrap());
}
#[tokio::test]async fn info_sync_account_mixed_original_recv_commits_before_matching_info_hint_and_never_writes(){Box::pin(mixed(0)).await;}
#[tokio::test]async fn info_sync_account_out_of_order_info_retains_same_holder_until_original_gap_and_atomic_online(){Box::pin(mixed(1)).await;}
#[tokio::test]async fn info_sync_account_invalid_info_rolls_back_text_and_cursor_but_keeps_original_sdk_healthy(){Box::pin(mixed(2)).await;}
#[tokio::test]
async fn info_sync_account_stop_retires_submitted_recv_before_late_info_and_all_original_joins(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(info_config(&cert.root,port,3000),100).await.unwrap().account;let line=account.line_id.clone();let(ready,seen)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;refresh(&mut peer,0,1,1).await;let p=group(&mut peer,n::im_data::RECV_METHOD).await;let(id,start,count)=receive_span(&p);assert_eq!((start,count),(1,1));ready.send(()).unwrap();
        let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"3");peer.send(&[reply(id,&mixed_records(&[(1,b"Info.Notice",BODY)]))]).await;
        loop{let p=peer.next().await.unwrap();assert_eq!(p[0],0,"retired recv cannot become an Info reply");let r=decode_request(&p[1..]).unwrap();if r.method==n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){peer.send(&[reply(r.call_id,&[0,1,0,0,0,0])]).await;break;}swap(&mut peer,&p).await;}while peer.next().await.is_some(){}
    }).await});
    bounded(manager.start(&line,account.revision,100)).await.unwrap();bounded(seen).await.unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();assert_eq!(super::super::count(&temp.database),0);assert_eq!(cursor(&temp.database,&line),(0,0,1));no_write_markers(&temp.database);assert!(!temp.database.with_conn(|c|n::account_runs::ownership_pending(c,&line)).unwrap());
}).await;}
