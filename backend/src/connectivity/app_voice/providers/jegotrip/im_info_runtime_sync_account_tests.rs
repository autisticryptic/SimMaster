//! Actions-only original Hosted consumer: Info recv, ordering, SQL and joins.
use super::*;
use super::super::info::{BODY,info_packet,no_write_markers,private_view};
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
