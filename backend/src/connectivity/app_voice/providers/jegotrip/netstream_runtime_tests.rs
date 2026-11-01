//! Actions-only physical TCP/TLS peers. No original SDK, provider or device.
use super::*;
use super::super::{native_channel::{self,HeaderMode,PacketKind,ChannelPacket,Records},
    native_router::decode_path_header,native_routing::{self,RouteAddress,Routing},
    netstream_records::{self,RecordReceiver,RecordSecurity},object_reference::{parse_object_reference,ReferenceTarget},
    packet_stream::encode_packet_stream_frame,router_connector::{RouterPolicy,RouterAnchor},router_runtime::RouterTimeouts,
    rpc_envelope::{decode_request,encode_reply},rpc_stream::Writer,session::HostingSession,tls_engine::tests::Certificates,tests::imported};
use std::{collections::BTreeMap,io,sync::Arc};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::{TcpListener,TcpStream},sync::oneshot};
use tokio_rustls::{rustls::{ServerConfig,version::TLS13},server::TlsStream};

pub(crate) fn owner()->AttemptOwner{let mut s=HostingSession::new("fixture-native-netstream".into());s.import(imported(),100).unwrap();s.begin_login(100).unwrap()}
pub(crate) fn policy(root:&str,port:u16)->RouterPolicy{RouterPolicy::supplied(&format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"),&[
 RouterAnchor{router_id:12001,host:"127.0.0.1",port,authority:"127.0.0.1",root,enable_sni:false}]).unwrap()}
pub(crate) fn timeouts()->RouterTimeouts{RouterTimeouts{connect:Duration::from_secs(2),handshake:Duration::from_secs(3),partial:Duration::from_secs(1),io:Duration::from_secs(2),startup:Duration::from_secs(5)}}
async fn read_packet(tls:&mut TlsStream<TcpStream>)->io::Result<Option<Vec<u8>>>{
 let n=match tls.read_u32().await{Ok(n)=>n as usize,Err(e) if matches!(e.kind(),io::ErrorKind::UnexpectedEof|io::ErrorKind::ConnectionReset)=>return Ok(None),Err(e)=>return Err(e)};
 assert!((1..16384).contains(&n));let mut bytes=vec![0;n];tls.read_exact(&mut bytes).await?;Ok(Some(bytes))
}
fn outer(sequence:u16,kind:u8,part:u8,body:&[u8])->Vec<u8>{
 let mut raw=sequence.to_be_bytes().to_vec();raw.extend_from_slice(&[0,10,0,0,0,kind|(part<<4)]);raw.extend_from_slice(body);
 encode_packet_stream_frame(&raw).unwrap().expose_to_transport().to_vec()
}
fn control(sequence:u16,id:i32,result:&[u8])->Vec<u8>{
 let rpc=encode_reply(id,0,result).unwrap();let mut body=vec![0;12];body.push(1);body.extend_from_slice(rpc.expose_to_transport());outer(sequence,1,1,&body)
}
fn node(sequence:u16)->Vec<u8>{let mut body=vec![0;12];body.push(1);outer(sequence,0,0,&body)}
pub(crate) fn channel(sequence:u16,kind:PacketKind,gap:u8,ack:u16,delivered:u16,body:&[u8])->Vec<u8>{
 native_channel::encode_packet(HeaderMode::Basic,&ChannelPacket{timestamp:None,sequence,kind,data_gap:gap,ack_sequence:ack,delivered_sequence:delivered,
  inclusive_ack:false,ack_mask:&[],body}).unwrap().expose_to_transport().to_vec()
}
pub(crate) struct Frame{pub wire:u16,pub root:u16,pub kind:PacketKind,pub previous:Option<u16>,pub body:Vec<u8>,pub ack:u16}
fn frame(bytes:&[u8])->Frame{
 let packet=native_channel::decode_packet(HeaderMode::Basic,netstream_records::unescape_channel(bytes).unwrap()).unwrap();let original=native_channel::original_packet(&packet).unwrap();
 let (previous,body)=if original.kind==PacketKind::Data{let(prev,body)=native_channel::previous_data(&original).unwrap();(Some(prev),body)}else{(None,original.body)};
 Frame{wire:packet.sequence,root:original.sequence,kind:original.kind,previous,body:body.to_vec(),ack:packet.ack_sequence}
}
pub(crate) struct Peer{pub tls:TlsStream<TcpStream>,port:u16,pub client_start:u16,pub client_high:u16,pub client_delivered:u16,pub next_channel:u16,pub last_data:u16,router_sequence:u16}
impl Peer{
 pub(crate) async fn accept_start(listener:TcpListener,config:Arc<ServerConfig>,drop_first:bool)->Self{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(config).accept(tcp).await.unwrap();
  let first=read_packet(&mut tls).await.unwrap().unwrap();let(_,body)=decode_path_header(&first).unwrap();let route=super::super::router_packets::decode_routed_body(body).unwrap();
  let request=decode_request(&route.payload[1..]).unwrap();assert_eq!(request.call_id,1);assert_eq!(request.method,b"clientConnect1.RouterClient.Common");
  let mut result=Writer::new();result.boolean(true).unwrap();result.u32(12345).unwrap();result.bytes(b"").unwrap();
  tls.write_all(&control(40,1,result.finish().expose_to_transport())).await.unwrap();tls.flush().await.unwrap();
  let mut router_sequence=41;let mut original_start=None;let mut dropped=false;let mut negotiated=false;
  loop{
   let raw=read_packet(&mut tls).await.unwrap().unwrap();let(header,body)=decode_path_header(&raw).unwrap();
   if header.packet_kind==1{
    let route=super::super::router_packets::decode_routed_body(body).unwrap();let request=decode_request(&route.payload[1..]).unwrap();let mut result=Writer::new();
    if request.method==b"clientResolveHosts.RouterClient.Common"{result.boolean(true).unwrap();result.u32(1).unwrap();result.bytes(b"42.7").unwrap();result.bytes(b"").unwrap();}
    else{assert_eq!(request.method,b"clientConnect2.RouterClient.Common");result.boolean(true).unwrap();result.u32(12345).unwrap();for _ in 0..8{result.u16(2).unwrap();}
     result.bytes(b"Router:tls -h ignored.invalid -p 1 -ri 9;").unwrap();result.bytes(b"").unwrap();negotiated=true;}
    tls.write_all(&control(router_sequence,request.call_id,result.finish().expose_to_transport())).await.unwrap();router_sequence+=1;tls.flush().await.unwrap();
   }else if header.fragment_part==0{
    assert_eq!(header.packet_kind,0);assert_eq!(body.last(),Some(&0));tls.write_all(&node(router_sequence)).await.unwrap();router_sequence+=1;tls.flush().await.unwrap();
   }else{
    assert!(negotiated);assert_eq!((header.packet_kind,header.fragment_part),(0,1));let route=native_routing::decode_routing_body(body).unwrap();
    assert_eq!((route.routing.destination.network,route.routing.destination.node,route.routing.destination.port),(42,7,98));assert_eq!(route.routing.source.mask,0);
    let start=frame(route.payload);assert_eq!(start.kind,PacketKind::Start);assert!(start.body.is_empty());
    assert!((0x4000..=0x7fff).contains(&start.root));if let Some(old)=original_start{assert_eq!(start.root,old);assert_ne!(start.wire,old);}else{original_start=Some(start.root);}
    if drop_first&&!dropped{dropped=true;continue;}
    return Self{tls,port:route.routing.source.port,client_start:start.root,client_high:start.wire,client_delivered:start.root.wrapping_sub(1),next_channel:0x9000,last_data:0x9000,router_sequence};
   }
  }
 }
 pub(crate) fn routed(&mut self,packet:&[u8])->Vec<u8>{
  let payload=netstream_records::escape_channel(packet).unwrap();let route=Routing{source:RouteAddress{mask:3,network:42,node:7,port:98,..Default::default()},
   destination:RouteAddress{port:self.port,..Default::default()},..Default::default()};let body=native_routing::encode_routing_body(&route,payload.expose_to_transport()).unwrap();
  let wire=outer(self.router_sequence,0,1,body.expose_to_transport());self.router_sequence+=1;wire
 }
 async fn write_channel(&mut self,packet:&[u8]){let wire=self.routed(packet);self.tls.write_all(&wire).await.unwrap();self.tls.flush().await.unwrap();}
 pub(crate) async fn read_channel(&mut self)->Option<Frame>{
  loop{let raw=match read_packet(&mut self.tls).await{Ok(Some(raw))=>raw,Ok(None)|Err(_)=>return None};let(header,body)=decode_path_header(&raw).unwrap();
   if header.packet_kind==3{assert_eq!(header.fragment_part,1);assert!(body.is_empty());continue;}
   if header.packet_kind==0&&header.fragment_part==0{if body.last()==Some(&0){let wire=node(self.router_sequence);self.router_sequence+=1;self.tls.write_all(&wire).await.unwrap();self.tls.flush().await.unwrap();}continue;}
   assert_eq!((header.packet_kind,header.fragment_part),(0,1));let route=native_routing::decode_routing_body(body).unwrap();let packet=frame(route.payload);self.client_high=packet.wire;return Some(packet);
  }
 }
 pub(crate) async fn start(&mut self){
  self.write_channel(&channel(self.next_channel,PacketKind::Start,0,self.client_high,self.client_start.wrapping_sub(1),&[])).await;self.next_channel+=1;
  let ack=self.read_channel().await.unwrap();assert_eq!(ack.kind,PacketKind::Probe);assert_eq!(ack.ack,0x9000);assert_eq!(ack.body,[0x89,0x87,0x22,0x83]);self.client_delivered=self.client_start;
 }
 pub(crate) async fn ack(&mut self,delivered:u16){
  self.client_delivered=delivered;
  self.write_channel(&channel(self.next_channel,PacketKind::Probe,0,self.client_high,delivered,&[0x89,0x87,0x22,0x83])).await;self.next_channel+=1;
 }
 async fn request(&mut self,drop_data:bool)->Vec<u8>{
  let mut dropped=false;let mut pending=BTreeMap::<u16,(u16,Vec<u8>)>::new();let mut delivered=self.client_start;
  let mut records=Records::default();let mut cipher=RecordReceiver::new(RecordSecurity::SeededXor);
  for _ in 0..128{
   let input=self.read_channel().await.unwrap();if input.kind==PacketKind::Probe{continue;}assert_eq!(input.kind,PacketKind::Data);
   if drop_data&&!dropped{dropped=true;continue;}
   let previous=input.previous.unwrap();
   if input.root>delivered{if let Some(old)=pending.get(&input.root){assert_eq!(old,&(previous,input.body));}else{pending.insert(input.root,(previous,input.body));}}
   let mut completed=None;
   loop{let next=pending.iter().find(|(_,value)|value.0==delivered).map(|(&seq,_)|seq);let Some(next)=next else{break};let(_,body)=pending.remove(&next).unwrap();
    for record in records.on_payload(&body).unwrap(){assert!(completed.is_none());completed=Some(cipher.decode(record.expose_to_transport()).unwrap().expose_to_transport().to_vec());}delivered=next;
   }
   self.ack(delivered).await;if let Some(record)=completed{return record;}
  }panic!("bounded native request did not complete")
 }
 async fn response(&mut self,malformed:bool){
  let first=self.next_channel;let second=first+1;let gap=(first-self.last_data-1).min(31) as u8;let previous=self.last_data;
  let first_body=|body:&[u8]|{let mut bytes=Vec::new();if gap==31{bytes.extend_from_slice(&previous.to_be_bytes());}bytes.extend_from_slice(body);bytes};
  // Independent first-direction seed1 and payload 'OK':4f^32=7d,4b^49=02.
  let encoded=[1,0,0,0,1,0x7d,0x02];let delivered=self.client_delivered;
  let group=if malformed{
   let mut body=vec![0xff];body.extend_from_slice(&encoded);
   let good=channel(first,PacketKind::Data,gap,self.client_high,delivered,&first_body(&body));let bad=channel(second,PacketKind::Data,0,self.client_high,delivered,&[0]);
   [self.routed(&good),self.routed(&bad)].concat()
  }else{
   let a=channel(first,PacketKind::Data,gap,self.client_high,delivered,&first_body(&[0xfe,1,0,0]));
   let b=channel(second,PacketKind::Data,0,self.client_high,delivered,&[0xff,0,1,0x7d,0x02]);
   // Last first, then first and duplicate first, one authenticated TLS group.
   [self.routed(&b),self.routed(&a),self.routed(&a)].concat()
  };
  self.last_data=second;self.next_channel=second+1;self.tls.write_all(&group).await.unwrap();self.tls.flush().await.unwrap();
 }
 pub(crate) async fn eof(mut self){while self.read_channel().await.is_some(){}}
}
pub(crate) fn reference(query:bool)->super::super::object_reference::ObjectReference{parse_object_reference(if query{b"AccountEntry:sarc -h arc@AccountEntry -p 98;"}else{b"AccountEntry:sarc -h 42.7 -p 98;"}).unwrap()}
// The only spawned client tasks are retained behind abort-on-drop guards.
// Peer futures are polled by join!, so assertion unwinding cannot detach them.
struct OwnedTestTask<T>(Option<tokio::task::JoinHandle<T>>);
impl<T> Drop for OwnedTestTask<T>{fn drop(&mut self){if let Some(task)=&self.0{task.abort();}}}
impl<T> OwnedTestTask<T>{
 fn new(task:tokio::task::JoinHandle<T>)->Self{Self(Some(task))}
 fn abort(&self){self.0.as_ref().unwrap().abort();}
 async fn join(mut self)->Result<T,tokio::task::JoinError>{
  // Borrow the handle during timeout; timeout never owns/drops/detaches it.
  let result=match tokio::time::timeout(Duration::from_secs(2),self.0.as_mut().unwrap()).await{
   Ok(result)=>result,
   Err(_)=>{self.abort();let _=self.0.as_mut().unwrap().await;self.0.take();panic!("owned task join timed out after abort and cleanup")}
  };self.0.take();result
 }
}
async fn echo_case(query:bool,drop_start:bool,drop_data:bool,malformed:bool){echo_case_with_delay(query,drop_start,drop_data,malformed,false).await;}
async fn echo_case_with_delay(query:bool,drop_start:bool,drop_data:bool,malformed:bool,slow:bool){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let mut request=vec![0];request.extend_from_slice(&vec![b'x';if slow{96*1024}else{4096}]);let expected=request.clone();
 let server=async move{tokio::time::timeout(Duration::from_secs(25),async move{let mut peer=Peer::accept_start(listener,config,drop_start).await;peer.start().await;
  assert_eq!(peer.request(drop_data).await,expected);peer.response(malformed).await;peer.eof().await;}).await.unwrap()};
 let client=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let router=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(query);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
  let mut path=NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(6),Duration::from_secs(1)).await.unwrap();assert!(path.ready_for(&original,&scope));
  if slow{super::super::router_runtime::tests::delay_original_io(&mut path.router,super::super::router_runtime::tests::DelayPoint::Flush,Duration::from_millis(50));}
  assert!(!path.send_records(&owner(),&scope,&[],Instant::now()).await.unwrap());assert!(scope.is_open());
  assert!(path.send_records(&original,&scope,&[TransportBytes::from_bytes(request)],Instant::now()+Duration::from_secs(20)).await.unwrap());
  let response=path.receive_records(&original,&scope,Instant::now()+Duration::from_secs(5)).await;
  if malformed{assert!(response.is_err());assert!(!scope.is_open());}
  else{let groups=response.unwrap().unwrap();let records:Vec<_>=groups.iter().flat_map(|group|path.records_for(&original,&scope,group).unwrap()).collect();
   assert_eq!(records.len(),1);assert_eq!(records[0].expose_to_transport(),[1,b'O',b'K']);path.close();assert!(path.records_for(&original,&scope,&groups[0]).is_none());}
  drop(path);assert!(!scope.is_open());
 };
 tokio::join!(server,client);
}

#[tokio::test]
async fn real_tcp_tls_query_channel_records_reassemble_out_of_order_and_deduplicate(){echo_case(true,false,false,false).await;}
#[tokio::test]
async fn read_only_retry_wake_retransmits_original_start_without_closing_tcp(){echo_case(false,true,false,false).await;}
#[tokio::test]
async fn lost_data_retransmits_same_encoded_record_without_restarting_direction_cipher(){echo_case(false,false,true,false).await;}
#[tokio::test]
async fn later_bad_channel_in_same_physical_tls_group_never_returns_earlier_good_record(){echo_case(false,false,false,true).await;}

#[tokio::test]
async fn cancelling_channel_startup_drops_original_socket_before_owning_task_join(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let(ready_tx,ready_rx)=oneshot::channel();
 let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let peer=Peer::accept_start(listener,config,false).await;ready_tx.send(()).unwrap();peer.eof().await;}).await.unwrap()};
 let controller=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let running=scope.clone();let task=OwnedTestTask::new(tokio::spawn(async move{let router=RouterPath::open(original,running,&p,&[],timeouts()).await.unwrap();
   let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(6),Duration::from_secs(1)).await}));
  ready_rx.await.unwrap();task.abort();assert!(task.join().await.unwrap_err().is_cancelled());assert!(!scope.is_open());
 };tokio::join!(server,controller);
}

async fn cancel_active(abort:bool){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=Peer::accept_start(listener,config,false).await;peer.start().await;peer.eof().await;}).await.unwrap()};
 let controller=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let running=scope.clone();let(ready_tx,ready_rx)=oneshot::channel();
  let task=OwnedTestTask::new(tokio::spawn(async move{let router=RouterPath::open(original.clone(),running.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
   let mut path=NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(6),Duration::from_secs(1)).await.unwrap();ready_tx.send(()).unwrap();
   path.receive_records(&original,&running,Instant::now()+Duration::from_secs(5)).await}));
  ready_rx.await.unwrap();if abort{task.abort();assert!(task.join().await.unwrap_err().is_cancelled());}else{scope.close();assert!(task.join().await.unwrap().is_err());}
  assert!(!scope.is_open());
 };tokio::join!(server,controller);
}
#[tokio::test]
async fn cancelling_active_read_keeps_scope_revocation_and_physical_drop_join_together(){cancel_active(true).await;}
#[tokio::test]
async fn scope_close_wakes_active_receiver_and_join_observes_original_socket_drop(){cancel_active(false).await;}

#[tokio::test]
async fn partial_channel_record_expires_despite_valid_probe_traffic_and_releases_tcp(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=Peer::accept_start(listener,config,false).await;peer.start().await;
  peer.write_channel(&channel(0x9001,PacketKind::Data,0,peer.client_high,peer.client_start,&[0xfe,1,0])).await;
  for seq in 0x9002..0x9006{tokio::time::sleep(Duration::from_millis(35)).await;let packet=channel(seq,PacketKind::Probe,0,peer.client_high,peer.client_start,&[0x89,0x87,0x22,0x83]);
   let bytes=peer.routed(&packet);if peer.tls.write_all(&bytes).await.is_err()||peer.tls.flush().await.is_err(){break;}}
  peer.eof().await;}).await.unwrap()};
 let client=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let router=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
  let mut path=NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(5),Duration::from_millis(100)).await.unwrap();let now=Instant::now();
  assert!(path.receive_records(&original,&scope,now+Duration::from_secs(3)).await.is_err());assert!(now.elapsed()<Duration::from_secs(2));assert!(!scope.is_open());drop(path);
 };tokio::join!(server,client);
}

#[tokio::test]
async fn original_startup_deadline_is_not_renewed_by_router_alive_traffic(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let server=async move{tokio::time::timeout(Duration::from_secs(8),async move{let mut peer=Peer::accept_start(listener,config,false).await;
  for _ in 0..120{let bytes=node(peer.router_sequence);peer.router_sequence+=1;
   if peer.tls.write_all(&bytes).await.is_err()||peer.tls.flush().await.is_err(){break;}
   tokio::time::sleep(Duration::from_millis(25)).await;
  }peer.eof().await;}).await.unwrap()};
 let client=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let router=RouterPath::open(original,scope.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
  let now=Instant::now();assert!(NetstreamPath::open(router,&endpoints[0],0,now+Duration::from_millis(350),Duration::from_secs(1)).await.is_err());
  assert!(now.elapsed()<Duration::from_secs(2));assert!(!scope.is_open());
 };tokio::join!(server,client);
}

#[tokio::test]
async fn sustained_slow_flush_reads_acknowledgements_before_retrying_and_drains_beyond_one_burst(){
 echo_case_with_delay(false,false,false,false,true).await;
}

async fn blocked_write_deadline(retry:bool){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
 let(first_tx,first_rx)=oneshot::channel();
 let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let mut peer=Peer::accept_start(listener,config,false).await;peer.start().await;
  if retry{let first=peer.read_channel().await.unwrap();assert_eq!(first.kind,PacketKind::Data);first_tx.send(()).unwrap();}else{drop(first_tx);}
  // No delayed first DATA or retry may reach the underlying socket after the
  //original root deadline, even when the newer receive call has3 seconds.
  while let Some(packet)=peer.read_channel().await{assert_ne!(packet.kind,PacketKind::Data,"expired original DATA reached physical IO");}
 }).await.unwrap()};
 let client=async move{
  let original=owner();let scope=RpcConnectionScope::fresh();let router=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
  let mut path=NetstreamPath::open(router,&endpoints[0],0,Instant::now()+Duration::from_secs(5),Duration::from_secs(1)).await.unwrap();
  let now=Instant::now();let end=now+Duration::from_millis(if retry{350}else{80});
  path.session.queue_record(&original,&scope,&[0,b'x'],end,now).unwrap();
  if retry{path.flush(now+Duration::from_secs(3)).await.unwrap();first_rx.await.unwrap();}else{drop(first_rx);}
  super::super::router_runtime::tests::delay_original_io(&mut path.router,super::super::router_runtime::tests::DelayPoint::Write,Duration::from_millis(300));
  assert!(path.receive_records(&original,&scope,now+Duration::from_secs(3)).await.is_err());assert!(now.elapsed()<Duration::from_secs(2));assert!(!scope.is_open());drop(path);
 };tokio::join!(server,client);
}
#[tokio::test]
async fn blocked_first_write_cannot_outlive_original_record_deadline(){blocked_write_deadline(false).await;}
#[tokio::test]
async fn blocked_retry_cannot_borrow_the_newer_receive_call_deadline(){blocked_write_deadline(true).await;}

pub(crate) fn delay_original_stream_write(path:&mut NetstreamPath,delay:Duration,skip:usize)->std::sync::Arc<std::sync::atomic::AtomicUsize>{
 super::super::router_runtime::tests::delay_original_io_after(&mut path.router,super::super::router_runtime::tests::DelayPoint::Write,delay,skip)
}
