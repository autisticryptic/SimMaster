//! Actions-only original TCP/TLS/Channel/Common integration, no provider/SDK.
use super::*;
use super::super::{common_compression::tests::reference_container,netstream_records::{RecordReceiver,RecordSender,RecordSecurity},
    native_channel::{self,PacketKind,Records},netstream_runtime::tests::{Peer,channel,owner,policy,timeouts,reference},
    object_reference::ReferenceTarget,router_runtime::RouterPath,tls_engine::tests::Certificates};
use std::{collections::VecDeque,sync::Arc};
use tokio::{io::AsyncWriteExt,net::TcpListener,sync::oneshot};
use tokio_rustls::rustls::{ServerConfig,version::TLS13};

pub(crate) struct CommonPeer{peer:Peer,sender:RecordSender,receiver:RecordReceiver,records:Records,last_client:u16,pending:VecDeque<Vec<u8>>}
impl CommonPeer{
 pub(crate) async fn open(listener:TcpListener,config:Arc<ServerConfig>)->Self{
  let mut peer=Peer::accept_start(listener,config,false).await;peer.start().await;let last_client=peer.client_start;
  Self{peer,sender:RecordSender::new(RecordSecurity::SeededXor,Some(0x12345678)).unwrap(),receiver:RecordReceiver::new(RecordSecurity::SeededXor),records:Records::default(),last_client,pending:VecDeque::new()}
 }
 pub(crate) async fn send(&mut self,packets:&[Vec<u8>]){
  let mut wire=Vec::new();
  for packet in packets{
   let encoded=self.sender.encode(packet).unwrap();for body in native_channel::encode_record(encoded.expose_to_transport(),900).unwrap(){
    let sequence=self.peer.next_channel;let distance=sequence.wrapping_sub(self.peer.last_data).wrapping_sub(1);let gap=distance.min(31) as u8;
    let mut payload=Vec::new();if gap==31{payload.extend_from_slice(&self.peer.last_data.to_be_bytes());}payload.extend_from_slice(body.expose_to_transport());
    let frame=channel(sequence,PacketKind::Data,gap,self.peer.client_high,self.peer.client_delivered,&payload);wire.extend_from_slice(&self.peer.routed(&frame));
    self.peer.last_data=sequence;self.peer.next_channel=self.peer.next_channel.wrapping_add(1);
   }
  }
  self.peer.tls.write_all(&wire).await.unwrap();self.peer.tls.flush().await.unwrap();
 }
 pub(crate) async fn next(&mut self)->Option<Vec<u8>>{
  loop{
   if let Some(value)=self.pending.pop_front(){return Some(value);}
   let frame=self.peer.read_channel().await?;if frame.kind==PacketKind::Probe{continue;}
   assert_eq!(frame.kind,PacketKind::Data);
   if frame.root.wrapping_sub(self.last_client) as i16>0{
    assert_eq!(frame.previous,Some(self.last_client));
    for record in self.records.on_payload(&frame.body).unwrap(){self.pending.push_back(self.receiver.decode(record.expose_to_transport()).unwrap().expose_to_transport().to_vec());}
    self.last_client=frame.root;
   }
   self.peer.ack(self.last_client).await;
  }
 }
 pub(crate) async fn no_more_common(mut self){assert!(self.next().await.is_none(),"unexpected Common control or business replay before EOF");}
}
fn reply_packet(payload:&[u8],compressed:bool)->Vec<u8>{
 let mut packet=vec![if compressed{0x11}else{1}];if compressed{packet.extend_from_slice(&reference_container(payload,1));}else{packet.extend_from_slice(payload);}packet
}
async fn open_client(p:super::super::router_connector::RouterPolicy,partial:Duration)->(CommonPath,AttemptOwner,RpcConnectionScope){
 let original=owner();let scope=RpcConnectionScope::fresh();let router=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
 let stream=NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(5),Duration::from_secs(2)).await.unwrap();
 (CommonPath::from_original(stream,partial,Some(32)).unwrap(),original,scope)
}
#[tokio::test]
async fn physical_common_compressed_call_and_ping_reply_keep_original_cipher_and_socket(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let response=reply_packet(b"original compressed response",true);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{
  let mut peer=CommonPeer::open(listener,config).await;let request=peer.next().await.unwrap();assert_eq!(request[0],0x10);
  assert_eq!(super::super::common_compression::decompress(&request[1..]).unwrap().expose_to_transport(),vec![b'x';4096]);
  peer.send(&[vec![2],response]).await;assert_eq!(peer.next().await.unwrap(),[14]);peer.no_more_common().await;
 }).await.unwrap()};
 let client=async move{
  let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;
  assert!(path.send_call(&original,&scope,CallKind::Request,&vec![b'x';4096],Instant::now()+Duration::from_secs(4)).await.unwrap());
  let group=path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(4)).await.unwrap().unwrap();
  let calls=path.calls_for(&original,&scope,&group).unwrap();assert_eq!(calls.len(),1);assert_eq!(calls[0].kind,CallKind::Reply);assert_eq!(calls[0].payload.expose_to_transport(),b"original compressed response");
  assert_eq!(path.delay_ms(&original,&scope),None);path.close();assert!(path.calls_for(&original,&scope,&group).is_none());
 };tokio::join!(server,client);
}
#[tokio::test]
async fn common_compressed_fragments_wait_for_complete_stream_before_delivery(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let container=reference_container(b"fragmented before compression decode",1);let mut first=vec![0x31];first.extend_from_slice(&(container.len() as u32).to_be_bytes());first.extend_from_slice(&container[..7]);let mut last=vec![3];last.extend_from_slice(&container[7..]);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[first]).await;tokio::time::sleep(Duration::from_millis(35)).await;peer.send(&[last]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let group=path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(4)).await.unwrap().unwrap();
  assert_eq!(path.calls_for(&original,&scope,&group).unwrap()[0].payload.expose_to_transport(),b"fragmented before compression decode");drop(path);assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn actual_local_ping_receives_one_pong_without_echo_loop_or_new_request_deadline(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;assert_eq!(peer.next().await.unwrap(),[2]);tokio::time::sleep(Duration::from_millis(20)).await;
  peer.send(&[vec![14],reply_packet(b"reply after pong",false)]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let deadline=Instant::now()+Duration::from_secs(4);assert!(path.send_ping(&original,&scope,deadline).await.unwrap());
  let group=path.receive_calls(&original,&scope,deadline).await.unwrap().unwrap();assert_eq!(path.calls_for(&original,&scope,&group).unwrap().len(),1);assert!(path.delay_ms(&original,&scope).is_some());drop(path);};tokio::join!(server,client);
}
#[tokio::test]
async fn unsolicited_common_pong_is_ignored_without_fabricating_rtt(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[vec![14],reply_packet(b"response",false)]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let group=path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(4)).await.unwrap().unwrap();assert!(path.calls_for(&original,&scope,&group).is_some());assert_eq!(path.delay_ms(&original,&scope),None);drop(path);};tokio::join!(server,client);
}
async fn reject_group(released:bool){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let mut bad=reply_packet(b"corrupt stream",true);let end=bad.len()-4;bad[end]^=0x80;if released{bad=vec![4];}
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[reply_packet(b"must not escape this group",false),vec![2],bad]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let result=path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(4)).await;assert!(result.is_err());
  if released{assert_eq!(result.err(),Some(CommonIoError::Control(ControlError::RemoteReleased)));}assert!(!scope.is_open());drop(path);};tokio::join!(server,client);
}
#[tokio::test]
async fn bad_later_compressed_call_suppresses_earlier_reply_and_pending_pong(){reject_group(false).await;}
#[tokio::test]
async fn remote_release_retires_original_tcp_and_suppresses_current_group_calls(){reject_group(true).await;}
#[tokio::test]
async fn common_fragment_expires_while_channel_probes_keep_physical_path_alive(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[vec![0x20,0,0,0,9,b'x']]).await;
  for _ in 0..6{tokio::time::sleep(Duration::from_millis(35)).await;let packet=channel(peer.peer.next_channel,PacketKind::Probe,0,peer.peer.client_high,peer.peer.client_delivered,&[0x89,0x87,0x22,0x83]);peer.peer.next_channel+=1;
   let bytes=peer.peer.routed(&packet);if peer.peer.tls.write_all(&bytes).await.is_err()||peer.peer.tls.flush().await.is_err(){break;}}
  peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_millis(100)).await;let begin=Instant::now();assert!(path.receive_calls(&original,&scope,begin+Duration::from_secs(3)).await.is_err());assert!(begin.elapsed()<Duration::from_secs(2));assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn foreign_owner_or_scope_cannot_send_ping_or_read_original_calls(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[reply_packet(b"private-owner-call",false)]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let foreign=owner();let now=Instant::now();assert!(!path.send_ping(&foreign,&scope,now).await.unwrap());assert!(path.receive_calls(&original,&RpcConnectionScope::fresh(),now).await.unwrap().is_none());
  let group=path.receive_calls(&original,&scope,now+Duration::from_secs(4)).await.unwrap().unwrap();assert!(path.calls_for(&foreign,&scope,&group).is_none());assert!(path.calls_for(&original,&RpcConnectionScope::fresh(),&group).is_none());assert!(!format!("{path:?} {group:?}").contains("private-owner-call"));drop(path);assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn common_receive_scope_close_drops_original_owner_without_detached_task(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);let(tx,rx)=oneshot::channel();
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let peer=CommonPeer::open(listener,config).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;assert!(tx.send(scope.clone()).is_ok());assert!(path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(4)).await.is_err());drop(path);};
 let cancel=async move{let scope:RpcConnectionScope=rx.await.unwrap();tokio::task::yield_now().await;scope.close();};tokio::join!(server,client,cancel);
}

async fn blocked_common_output(operation:u8){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);let(tx,rx)=oneshot::channel();
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;rx.await.unwrap();
  if operation==2{peer.send(&[vec![2]]).await;}
  peer.no_more_common().await;}).await.unwrap()};
 let client=async move{
  let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;
  // For required Pong, the first write is only its Channel receipt ACK.
  //Delay the SECOND write to prove the Common-response branch is exercised.
  let pauses=super::super::netstream_runtime::tests::delay_original_stream_write(&mut path.stream,Duration::from_millis(300),usize::from(operation==2));
  tx.send(()).unwrap();let now=Instant::now();let end=now+Duration::from_millis(100);
  if operation==3{
   let result={let mut pending=Box::pin(path.send_ping(&original,&scope,now+Duration::from_secs(3)));
    tokio::select!{result=&mut pending=>Some(result),_=tokio::time::sleep(Duration::from_millis(35))=>None}
   };assert!(result.is_none());assert!(!scope.is_open());
  }else{
   let failed=match operation{0=>path.send_ping(&original,&scope,end).await.is_err(),1=>path.send_call(&original,&scope,CallKind::Request,b"original-call",end).await.is_err(),
    2=>path.receive_calls(&original,&scope,end).await.is_err(),_=>unreachable!()};assert!(failed);assert!(!scope.is_open());
  }
  assert!(pauses.load(std::sync::atomic::Ordering::Acquire)>0);assert!(now.elapsed()<Duration::from_secs(2));path.close();assert_eq!(path.controls.deadline(),None);drop(path);
 };tokio::join!(server,client);
}
#[tokio::test]
async fn common_ping_write_obeys_original_deadline_and_clears_pending_state(){blocked_common_output(0).await;}
#[tokio::test]
async fn common_call_write_cannot_cross_its_original_operation_deadline(){blocked_common_output(1).await;}
#[tokio::test]
async fn required_common_pong_write_expires_without_sending_after_original_deadline(){blocked_common_output(2).await;}
#[tokio::test]
async fn cancelling_borrowed_common_ping_revokes_scope_then_explicit_owner_drop_releases_tcp(){blocked_common_output(3).await;}
#[tokio::test]
async fn same_scope_wrong_live_path_brand_and_expired_group_cannot_expose_calls(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[reply_packet(b"brand-bound",false)]).await;peer.no_more_common().await;}).await.unwrap()};
 let client=async move{let(mut path,original,scope)=open_client(p,Duration::from_secs(2)).await;let mut group=path.receive_calls(&original,&scope,Instant::now()+Duration::from_secs(3)).await.unwrap().unwrap();
  assert!(path.calls_for(&original,&scope,&group).is_some());let another=Arc::new(AtomicBool::new(true));let old=std::mem::replace(&mut group.live,Arc::downgrade(&another));
  assert!(path.calls_for(&original,&scope,&group).is_none());group.live=old;group.calls[0].deadline=Instant::now();assert!(path.calls_for(&original,&scope,&group).is_none());drop(path);
 };tokio::join!(server,client);
}
