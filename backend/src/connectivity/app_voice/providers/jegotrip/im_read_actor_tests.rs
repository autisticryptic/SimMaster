//! Actions-only original Router/TLS/Common IM read IO/cancel/timeout/Join.
//! No device, original SDK execution, arbitrary routing or remote write.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{native_receiver::{NativeReceiverConfig,NativeReceiverPolicy,NativeReceiverSetup},im_delivery::NativeImConfig,
    rpc_im::{ImQuery,ImReplyData},im_data::ImBox,im_inbox::ImReceiptGate};
use std::sync::atomic::{AtomicBool,Ordering};
#[path="im_received_io_tests.rs"]
mod received_writes;
#[path="im_user_read_io_tests.rs"]
mod user_reads;
#[path="im_send_io_tests.rs"]
mod sends;
#[path="relation_io_tests.rs"]
mod relations;
#[path="im_info_runtime_io_tests.rs"]
mod info;
#[path="group_directory_io_tests.rs"]
mod directory;
async fn bounded<F:std::future::Future>(f:F)->F::Output{tokio::time::timeout(Duration::from_secs(12),f).await.expect("original IM IO fixture budget")}
fn refresh()->Arc<ImQuery>{Arc::new(ImQuery::refresh(&[],-1).unwrap())}
fn config(enabled:bool)->NativeReceiverConfig{NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:enabled.then_some(NativeImConfig{notification_timeout_ms:3000,receive_info:false,directory_queries:false})}}
async fn auth(handle:&RuntimeHandle,events:&mut mpsc::Receiver<RuntimeEvent>,original:&AttemptOwner,scope:&RpcConnectionScope,enabled:bool){
    let setup=NativeReceiverSetup::capture(NativeReceiverPolicy::capture(&config(enabled)).unwrap(),true,None).unwrap();let commands=handle.command_handle();commands.arm_native_receivers(original,scope,setup).await.unwrap();authenticated(handle,events,original,scope).await;commands.configure_native_receivers(original,scope,7).await.unwrap();
}
async fn peer_login(peer:&mut CommonPeer){let p=peer.next().await.unwrap();let request=decode_request(&p[1..]).unwrap();assert_eq!(request.method,n::rpc_login::LOGIN_SESSION_METHOD.as_bytes());peer.send(&[reply(request.call_id,&login_result(true))]).await;}
fn query(packet:&[u8],method:&str)->i32{assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,method.as_bytes());assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");request.call_id}
fn status()->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(0).unwrap();w.i64(4294967297).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn received(indices:&[i64])->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(indices.len()as u32).unwrap();for n in indices{w.i64(*n).unwrap();w.i64(7).unwrap();w.bytes(b"private-native-sender").unwrap();w.bytes(b"Text").unwrap();w.bytes(br#"{"text":"private IM IO"}"#).unwrap();w.properties(&[]).unwrap();w.properties(&[]).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
async fn peer_logout(peer:&mut CommonPeer){let p=peer.next().await.unwrap();let r=decode_request(&p[1..]).unwrap();assert_eq!(r.method,n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(),"no retired read replay");peer.send(&[reply(r.call_id,&[0,1,0,0,0,0])]).await;assert!(peer.next().await.is_none());}
fn enqueue(handle:&RuntimeHandle,original:&AttemptOwner,scope:&RpcConnectionScope,gate:Arc<ImReceiptGate>,interest:Arc<ImReadInterest>)->(oneshot::Receiver<Result<RuntimeReceipt,RuntimeError>>,oneshot::Receiver<Result<OwnedImReply,RuntimeError>>){let(reply,data)=oneshot::channel();let receipt=handle.command_handle().enqueue(original,scope,Operation::ImRead(Box::new(ImReadWork{query:refresh(),gate,interest,reply}))).unwrap();(receipt,data)}
#[test]
fn im_interest_queued_cancel_cannot_write_or_close_other_sdk_operations(){let scope=RpcConnectionScope::fresh();let gate=ImReceiptGate::fresh();let interest=ImReadInterest::original(&scope);interest.cancel();assert!(!interest.begin_write(&gate));assert!(interest.is_retired());assert!(scope.is_open());}
#[test]
fn im_interest_partial_write_cancel_closes_original_scope_and_cannot_be_submitted_later(){let scope=RpcConnectionScope::fresh();let interest=ImReadInterest::original(&scope);assert!(interest.begin_write(&ImReceiptGate::fresh()));interest.cancel();assert!(!scope.is_open());assert!(!interest.submitted());}
#[test]
fn im_interest_full_submission_cancel_retires_only_read_wait_not_sdk_io(){let scope=RpcConnectionScope::fresh();let interest=ImReadInterest::original(&scope);assert!(interest.begin_write(&ImReceiptGate::fresh()));assert!(interest.submitted());interest.cancel();assert!(interest.is_retired());assert!(scope.is_open());assert!(!interest.submitted());}
#[tokio::test]
async fn im_actor_original_refresh_and_receive_use_real_io_but_do_not_emit_fake_message_or_ready(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);
    let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let packet=peer.next().await.unwrap();let id=query(&packet,n::im_status::REFRESH_METHOD);let req=decode_request(&packet[1..]).unwrap();let mut r=Reader::versioned(req.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");peer.send(&[reply(id,&status())]).await;
        let packet=peer.next().await.unwrap();let id=query(&packet,n::im_data::RECV_METHOD);let req=decode_request(&packet[1..]).unwrap();let mut r=Reader::versioned(req.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"P2P/1_2");assert_eq!(r.i64().unwrap(),7);assert_eq!(r.u32().unwrap(),2);r.finish().unwrap();peer.send(&[reply(id,&received(&[7,8]))]).await;peer_logout(&mut peer).await;}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;let gate=ImReceiptGate::fresh();let commands=handle.command_handle();
    let data=bounded(commands.read_im(&original,&scope,refresh(),gate.clone())).await.unwrap();assert!(data.matches(&original,&scope));let ImReplyData::Refresh(value)=data.result().unwrap()else{panic!("refresh")};assert!(value.accepted);assert_eq!(value.update_time,4294967297);
    let data=bounded(commands.read_im(&original,&scope,Arc::new(ImQuery::receive(ImBox::parse(b"P2P/1_2").unwrap(),7,2).unwrap()),gate)).await.unwrap();let ImReplyData::Receive(value)=data.result().unwrap()else{panic!("receive")};assert_eq!(value.messages.len(),2);assert_eq!(value.messages[1].index,8);assert!(events.try_recv().is_err());let closed=bounded(handle.logout(&original,&scope,&[])).await.unwrap();assert!(closed.io_released());bounded(serving).await.unwrap();
}).await;}
async fn retired_wait(parent:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let(go,wait)=oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let packet=peer.next().await.unwrap();let id=query(&packet,n::im_status::REFRESH_METHOD);wait.await.unwrap();
        // The late result is intentionally malformed: a retired read ID must
        // be ignored without decoding it or breaking healthy SDK logout.
        peer.send(&[reply(id,&[0])]).await;peer_logout(&mut peer).await;}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;let gate=ImReceiptGate::fresh();let interest=ImReadInterest::original(&scope);let(receipt,data)=enqueue(&handle,&original,&scope,gate.clone(),interest.clone());
    let sent=bounded(receipt).await.unwrap().unwrap();assert!(sent.matches(&original,&scope));assert!(!interest.is_writing());if parent{gate.retire();}else{interest.cancel();}go.send(()).unwrap();assert_eq!(bounded(data).await.unwrap().err(),Some(RuntimeError::Cancelled));assert!(scope.is_open());assert!(events.try_recv().is_err());let closed=bounded(handle.logout(&original,&scope,&[])).await.unwrap();assert!(closed.io_released());bounded(serving).await.unwrap();
}
#[tokio::test]async fn im_actor_cancel_submitted_read_ignores_late_invalid_reply_and_preserves_logout_join(){Box::pin(retired_wait(false)).await;}
#[tokio::test]async fn im_actor_parent_stop_retires_submitted_read_without_replay_or_sdk_disconnect(){Box::pin(retired_wait(true)).await;}
#[tokio::test]
async fn im_actor_dropping_submitted_reader_wait_keeps_original_sdk_io_and_no_replayed_request(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let(go,wait)=oneshot::channel();let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let packet=peer.next().await.unwrap();let id=query(&packet,n::im_status::REFRESH_METHOD);wait.await.unwrap();peer.send(&[reply(id,&status())]).await;peer_logout(&mut peer).await;}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;let interest=ImReadInterest::original(&scope);let(receipt,data)=enqueue(&handle,&original,&scope,ImReceiptGate::fresh(),interest.clone());let sent=bounded(receipt).await.unwrap().unwrap();assert!(sent.matches(&original,&scope));let waiting=ImReadWait::original(interest);drop(data);drop(waiting);go.send(()).unwrap();assert!(scope.is_open());let closed=bounded(handle.logout(&original,&scope,&[])).await.unwrap();assert!(closed.io_released());bounded(serving).await.unwrap();
}).await;}
#[tokio::test]
async fn im_actor_read_timeout_retires_interest_only_and_still_joins_sdk_logout(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let(go,wait)=oneshot::channel();let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let packet=peer.next().await.unwrap();let id=query(&packet,n::im_status::REFRESH_METHOD);wait.await.unwrap();peer.send(&[reply(id,&[0])]).await;peer_logout(&mut peer).await;}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;assert_eq!(bounded(handle.command_handle().read_im(&original,&scope,refresh(),ImReceiptGate::fresh())).await.err(),Some(RuntimeError::Timeout));assert!(scope.is_open());go.send(()).unwrap();let closed=bounded(handle.logout(&original,&scope,&[])).await.unwrap();assert!(closed.io_released());bounded(serving).await.unwrap();
}).await;}
async fn blocked_read(parent:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;assert!(peer.next().await.is_none(),"no blocked IM query escapes");}).await});
    let original=owner();let mut conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let blocked=Arc::new(AtomicBool::new(false));let pauses=n::common_runtime::tests::gate_im_read_write(conn.common.as_mut().unwrap(),blocked.clone());let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;blocked.store(true,Ordering::SeqCst);
    let gate=ImReceiptGate::fresh();let interest=ImReadInterest::original(&scope);let(mut receipt,_data)=enqueue(&handle,&original,&scope,gate.clone(),interest.clone());assert!(tokio::time::timeout(Duration::from_millis(35),&mut receipt).await.is_err());assert!(interest.is_writing());assert!(pauses.load(Ordering::SeqCst)>0);if parent{gate.retire();}else{drop(ImReadWait::original(interest));}
    bounded(scope.closed()).await;let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Cancelled);bounded(serving).await.unwrap();
}
#[tokio::test]async fn im_actor_partial_read_write_cancel_closes_scope_and_retains_real_io_join(){Box::pin(blocked_read(false)).await;}
#[tokio::test]async fn im_actor_parent_revocation_during_read_write_closes_original_scope_before_more_bytes(){Box::pin(blocked_read(true)).await;}
#[tokio::test]
async fn im_actor_remote_range_error_is_not_read_success_and_releases_original_io(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let packet=peer.next().await.unwrap();let id=query(&packet,n::im_data::RECV_METHOD);peer.send(&[reply(id,&received(&[9]))]).await;assert!(peer.next().await.is_none());}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,true).await;assert!(bounded(handle.command_handle().read_im(&original,&scope,Arc::new(ImQuery::receive(ImBox::parse(b"P2P/1_2").unwrap(),7,2).unwrap()),ImReceiptGate::fresh())).await.is_err());let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Protocol);bounded(serving).await.unwrap();
}).await;}
#[tokio::test]
async fn im_actor_uninstalled_receiver_foreign_scope_and_writes_do_not_allocate_or_send_group_packets(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let p=peer.next().await.unwrap();assert_eq!(decode_request(&p[1..]).unwrap().call_id,2);let req=decode_request(&p[1..]).unwrap();assert_eq!(req.method,n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());peer.send(&[reply(2,&[0,1,0,0,0,0])]).await;assert!(peer.next().await.is_none());}).await});
    let original=owner();let conn=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&original,&scope,false).await;let commands=handle.command_handle();assert_eq!(commands.read_im(&owner(),&scope,refresh(),ImReceiptGate::fresh()).await.err(),Some(RuntimeError::Scope));assert_eq!(commands.read_im(&original,&RpcConnectionScope::fresh(),refresh(),ImReceiptGate::fresh()).await.err(),Some(RuntimeError::Scope));assert_eq!(commands.read_im(&original,&scope,refresh(),ImReceiptGate::fresh()).await.err(),Some(RuntimeError::InvalidConfiguration));assert_eq!(commands.read_im(&original,&scope,Arc::new(ImQuery::set_received(ImBox::parse(b"P2P/1_2").unwrap(),7).unwrap()),ImReceiptGate::fresh()).await.err(),Some(RuntimeError::InvalidConfiguration));assert!(scope.is_open());assert!(bounded(handle.logout(&original,&scope,&[])).await.unwrap().io_released());bounded(serving).await.unwrap();
}).await;}
