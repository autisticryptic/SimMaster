//! Actions-only real loopback TCP/TLS/Router lifecycle, no original SDK/provider.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{router_connector::RouterAnchor,
    tls_engine::tests::Certificates,session::HostingSession,tests::imported,
    router_packets::{decode_routed_body,encode_routed_body,encode_router_packets,RouterAddress,RouterHeader,RouterRouting},
    native_router::decode_path_header,packet_stream::encode_packet_stream_frame,
    rpc_envelope::{decode_request,encode_reply},rpc_stream::Writer,transport_packets::{CallKind,encode_call_packets,TLS_FRAGMENT_THRESHOLD}};
use tokio::net::TcpListener;
fn owner()->AttemptOwner{let mut s=HostingSession::new("fixture-router-runtime".into());s.import(imported(),100).unwrap();s.begin_login(100).unwrap()}
fn policy(root:&str,port:u16)->RouterPolicy{RouterPolicy::supplied(&format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"),&[
 RouterAnchor{router_id:12001,host:"127.0.0.1",port,authority:"127.0.0.1",root,enable_sni:false}]).unwrap()}
fn timeouts()->RouterTimeouts{RouterTimeouts{connect:Duration::from_secs(2),handshake:Duration::from_secs(3),partial:Duration::from_secs(1),io:Duration::from_secs(2),startup:Duration::from_secs(5)}}
async fn packet<S:tokio::io::AsyncRead+Unpin>(io:&mut S)->Vec<u8>{let n=io.read_u32().await.unwrap() as usize;assert!((1..16384).contains(&n));let mut v=vec![0;n];io.read_exact(&mut v).await.unwrap();v}
fn reply()->Vec<u8>{
 let mut result=Writer::new();result.boolean(true).unwrap();result.u32(12345).unwrap();result.bytes(b"").unwrap();let result=result.finish();let rpc=encode_reply(1,0,result.expose_to_transport()).unwrap();
 let common=encode_call_packets(CallKind::Reply,rpc.expose_to_transport(),TLS_FRAGMENT_THRESHOLD).unwrap();
 let route=RouterRouting{tag:0,flags:0,path_words:&[],address1:RouterAddress{mask:0,label:0,bytes:&[0,0]},address2:RouterAddress{mask:0,label:0,bytes:&[0,0]}};
 let body=encode_routed_body(&route,common[0].expose_to_transport()).unwrap();let packets=encode_router_packets(RouterHeader{sequence:40,word1:10,word2:0,byte6:0,packet_kind:1,fragment_part:1},body.expose_to_transport(),1460).unwrap();
 encode_packet_stream_frame(packets[0].expose_to_transport()).unwrap().expose_to_transport().to_vec()
}
#[tokio::test]
async fn production_tcp_tls_router_control_and_original_drop_release_real_socket(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();let bytes=packet(&mut tls).await;
  let(header,body)=decode_path_header(&bytes).unwrap();assert_eq!(header.packet_kind,1);let route=decode_routed_body(body).unwrap();assert_eq!(&body[..12],&[0;12]);
  let req=decode_request(&route.payload[1..]).unwrap();assert_eq!(req.method,b"clientConnect1.RouterClient.Common");assert_eq!(req.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"Router");
  assert!(!req.metadata.iter().any(|p|p.key==b"pwd"||p.key==b"token"));tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();
  let mut bytes=[0;1];assert!(matches!(tls.read(&mut bytes).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let owner=owner();let scope=RpcConnectionScope::fresh();let path=RouterPath::open(owner,scope.clone(),&p,&[],timeouts()).await.unwrap();assert_eq!(path.ids(),Some((12001,12345)));drop(path);assert!(!scope.is_open());worker.await.unwrap();
}
#[tokio::test]
async fn cancelling_startup_closes_original_scope_and_tcp_before_join_completes(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());
 let owner=owner();let scope=RpcConnectionScope::fresh();let running=scope.clone();let task=tokio::spawn(async move{RouterPath::open(owner,running,&p,&[],timeouts()).await});
 let(mut tcp,_)=listener.accept().await.unwrap();let mut bytes=[0;4096];let n=tcp.read(&mut bytes).await.unwrap();assert!(n>0);assert_eq!(bytes[0],22);
 task.abort();assert!(task.await.unwrap_err().is_cancelled());assert!(!scope.is_open());
 let ended=tokio::time::timeout(Duration::from_secs(2),async{loop{match tcp.read(&mut bytes).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}}).await;assert!(ended.is_ok());
}
#[tokio::test]
async fn wrong_endpoint_root_never_sends_router_credentials_or_connect_request(){
 let cert=Certificates::generate("IP:127.0.0.1");let other=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&other.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move {let(tcp,_)=listener.accept().await.unwrap();let result=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await;if let Ok(mut tls)=result{let mut bytes=[0;8];assert!(matches!(tls.read(&mut bytes).await,Ok(0)|Err(_)));}});
 let owner=owner();let scope=RpcConnectionScope::fresh();let result=RouterPath::open(owner,scope.clone(),&p,&[],timeouts()).await;assert!(matches!(result,Err(RouterIoError::Tls(TlsError::HandshakeFailed))));assert!(!scope.is_open());worker.await.unwrap();
}
#[tokio::test]
async fn startup_budget_does_not_extend_when_tcp_peer_drips_tls_header(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());
 let worker=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();let mut bytes=[0;4096];let _=tcp.read(&mut bytes).await;for b in [22,3,3]{if tcp.write_all(&[b]).await.is_err(){break;}tokio::time::sleep(Duration::from_millis(40)).await;}});
 let mut bounds=timeouts();bounds.startup=Duration::from_millis(90);let owner=owner();let scope=RpcConnectionScope::fresh();let begin=Instant::now();
 let result=RouterPath::open(owner,scope.clone(),&p,&[],bounds).await;assert!(result.is_err());assert!(!scope.is_open());assert!(begin.elapsed()<Duration::from_secs(2));worker.await.unwrap();
}

fn node_wire(sequence:u16,opcode:u8)->Vec<u8>{
 let mut raw=vec![];raw.extend_from_slice(&sequence.to_be_bytes());raw.extend_from_slice(&[0,10,0,0,0,0]);
 raw.extend_from_slice(&[0;12]);raw.push(opcode);
 encode_packet_stream_frame(&raw).unwrap().expose_to_transport().to_vec()
}
fn channel_wire(sequence:u16,port:u16)->Vec<u8>{
 let mut raw=vec![];raw.extend_from_slice(&sequence.to_be_bytes());raw.extend_from_slice(&[0,11,0,0,0,0x10]);
 raw.extend_from_slice(&[0,0,0,0,0,4,0,0,0,42,0xff,0xff,0,98]);raw.extend_from_slice(&port.to_be_bytes());
 raw.extend_from_slice(b"unaccepted-native-channel");encode_packet_stream_frame(&raw).unwrap().expose_to_transport().to_vec()
}
fn assert_node_packet(raw:&[u8],opcode:u8){
 assert_eq!(raw.len(),21);assert_eq!(raw[7],0);assert_eq!(&raw[8..20],&[0;12]);assert_eq!(raw[20],opcode);
}

#[tokio::test]
async fn production_node_probe_and_same_group_data_bind_only_original_listen_and_socket(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();
  let probe=packet(&mut tls).await;assert_node_packet(&probe,0);
  let group=[node_wire(41,1),channel_wire(42,12000)].concat();tls.write_all(&group).await.unwrap();tls.flush().await.unwrap();
  let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 let foreign=owner();assert!(path.listen(&foreign,&scope,ListenProtocol::Arc,Some(12000),false,Instant::now()).await.unwrap().is_none());
 let lease=path.listen(&original,&scope,ListenProtocol::Arc,Some(12000),false,Instant::now()+Duration::from_secs(3)).await.unwrap().unwrap();
 let address=lease.address_for(&original,&scope).unwrap();assert_eq!((address.network,address.node,address.port),(12001,12345,12000));
 let data=path.take_deliveries(&original,&scope).unwrap();assert_eq!(data.len(),1);
 assert_eq!(data[0].for_lease(&original,&scope,&lease).unwrap().1,b"unaccepted-native-channel");
 assert!(path.take_deliveries(&foreign,&scope).is_none());drop(path);assert!(data[0].for_lease(&original,&scope,&lease).is_none());
 assert!(!scope.is_open());worker.await.unwrap();
}

#[tokio::test]
async fn initial_connect_group_node_probe_is_processed_before_waiting_for_another_read(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;let group=[reply(),node_wire(41,0)].concat();tls.write_all(&group).await.unwrap();tls.flush().await.unwrap();
  let answer=packet(&mut tls).await;assert_node_packet(&answer,1);
  let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 let lease=path.listen(&original,&scope,ListenProtocol::Arc,Some(12000),false,Instant::now()+Duration::from_secs(3)).await.unwrap().unwrap();
 assert!(lease.address_for(&original,&scope).is_some());assert!(path.take_deliveries(&original,&scope).unwrap().is_empty());
 assert!(path.last_node_probe.is_none());drop(path);worker.await.unwrap();
}

#[tokio::test]
async fn logical_listen_deadline_closes_original_socket_without_accepting_connect_as_host_proof(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();assert_node_packet(&packet(&mut tls).await,0);
  let mut end=[0;128];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 let now=Instant::now();assert!(path.listen(&original,&scope,ListenProtocol::Arc,None,false,now+Duration::from_millis(100)).await.is_err());
 assert!(now.elapsed()<Duration::from_secs(2));assert!(path.io.is_none());assert!(!scope.is_open());worker.await.unwrap();
}

#[tokio::test]
async fn node_control_drip_does_not_turn_tls_partial_expiry_into_a_keepalive_wake(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();let _probe=packet(&mut tls).await;
  let raw=node_wire(41,1);for byte in &raw[..4]{if tls.write_all(&[*byte]).await.is_err(){break;}if tls.flush().await.is_err(){break;}tokio::time::sleep(Duration::from_millis(40)).await;}
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut bounds=timeouts();bounds.partial=Duration::from_millis(80);
 let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],bounds).await.unwrap();let now=Instant::now();
 assert!(path.listen(&original,&scope,ListenProtocol::Arc,None,false,now+Duration::from_millis(800)).await.is_err());
 assert!(now.elapsed()<Duration::from_millis(600));assert!(!scope.is_open());assert!(path.io.is_none());worker.await.unwrap();
}

#[tokio::test]
async fn node_probe_retry_uses_same_original_path_sequence_without_renewing_listen_budget(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(10),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();
  let first=packet(&mut tls).await;assert_node_packet(&first,0);let second=packet(&mut tls).await;assert_node_packet(&second,0);
  assert_eq!(u16::from_be_bytes([second[0],second[1]]),u16::from_be_bytes([first[0],first[1]]).wrapping_add(1));
  tls.write_all(&node_wire(41,1)).await.unwrap();tls.flush().await.unwrap();let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 let deadline=Instant::now()+Duration::from_secs(7);let lease=path.listen(&original,&scope,ListenProtocol::Arc,None,false,deadline).await.unwrap().unwrap();
 assert!(Instant::now()<deadline);assert!(scope.is_open());assert!(lease.address_for(&original,&scope).is_some());drop(path);worker.await.unwrap();
}

#[tokio::test]
async fn closing_scope_while_listen_waits_retires_owned_socket_before_task_join(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let (ready_tx,ready_rx)=tokio::sync::oneshot::channel();
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let _connect=packet(&mut tls).await;tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();assert_node_packet(&packet(&mut tls).await,0);
  ready_tx.send(()).unwrap();let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let running=scope.clone();
 let task=tokio::spawn(async move{
  let mut path=RouterPath::open(original.clone(),running.clone(),&p,&[],timeouts()).await.unwrap();
  path.listen(&original,&running,ListenProtocol::Arc,None,false,Instant::now()+Duration::from_secs(5)).await
 });
 ready_rx.await.unwrap();scope.close();let result=tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap();
 assert!(result.is_err());assert!(!scope.is_open());worker.await.unwrap();
}

fn rpc_result_wire(id:i32,result:&[u8])->Vec<u8>{
 let rpc=encode_reply(id,0,result).unwrap();let common=encode_call_packets(CallKind::Reply,rpc.expose_to_transport(),TLS_FRAGMENT_THRESHOLD).unwrap();assert_eq!(common.len(),1);
 let mut raw=vec![0,60,0,10,0,0,0,0x11];raw.extend_from_slice(&[0;12]);raw.extend_from_slice(common[0].expose_to_transport());
 encode_packet_stream_frame(&raw).unwrap().expose_to_transport().to_vec()
}
async fn original_peer_flow(query:bool){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(12),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();
  let first=packet(&mut tls).await;let(_,body)=decode_path_header(&first).unwrap();let route=decode_routed_body(body).unwrap();
  let connect=decode_request(&route.payload[1..]).unwrap();let nonce=connect.arguments[9..13].to_vec();
  tls.write_all(&reply()).await.unwrap();tls.flush().await.unwrap();let mut resolved=!query;let mut negotiated=false;let mut delivered=false;
  while !delivered {
   let raw=packet(&mut tls).await;let(header,body)=decode_path_header(&raw).unwrap();
   if header.packet_kind==1 {
    let route=decode_routed_body(body).unwrap();let request=decode_request(&route.payload[1..]).unwrap();let mut result=Writer::new();
    if request.method==b"clientResolveHosts.RouterClient.Common" {
     assert!(query);assert!(!resolved);assert_eq!(request.call_id,2);assert_eq!(&request.arguments[9..],b"AccountEntry");
     result.boolean(true).unwrap();result.u32(1).unwrap();result.bytes(b"42.7").unwrap();result.bytes(b"").unwrap();resolved=true;
    }else{
     assert!(resolved);assert_eq!(request.method,b"clientConnect2.RouterClient.Common");assert_eq!(request.call_id,if query{3}else{2});
     assert_eq!(&request.arguments[5..9],&12345u32.to_be_bytes());assert_eq!(&request.arguments[9..13],nonce.as_slice());
     assert_eq!(&request.arguments[13..],&[0,0,0x2e,0xe1,0,0,0,42,0,0,0,7]);
     result.boolean(true).unwrap();result.u32(12345).unwrap();for value in [1,2,3,4,5,6,7,8]{result.u16(value).unwrap();}
     // This unapproved hint must not change the original live TLS socket.
     result.bytes(b"Router:tls -h ignored.invalid -p 1 -ri 9;").unwrap();result.bytes(b"").unwrap();negotiated=true;
    }
    tls.write_all(&rpc_result_wire(request.call_id,result.finish().expose_to_transport())).await.unwrap();tls.flush().await.unwrap();
   }else if header.fragment_part==0{
    assert_node_packet(&raw,0);tls.write_all(&node_wire(61,1)).await.unwrap();tls.flush().await.unwrap();
   }else{
    assert!(negotiated);assert_eq!((header.packet_kind,header.fragment_part),(0,1));
    let route=crate::connectivity::app_voice::providers::jegotrip::native_routing::decode_routing_body(body).unwrap();
    assert_eq!((route.routing.source.mask,route.routing.source.port),(0,12000));
    assert_eq!((route.routing.destination.network,route.routing.destination.node,route.routing.destination.port),(42,7,98));
    assert_eq!((route.routing.destination.network_tag,route.routing.destination.node_tag),(1,5));assert_eq!(route.routing.tag,77);
    assert_eq!(route.payload,b"channel-frame-not-sdk-ready");delivered=true;
   }
  }
  let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 let reference=crate::connectivity::app_voice::providers::jegotrip::object_reference::parse_object_reference(
  if query{b"AccountEntry:sarc -h arc@AccountEntry -p 98;"}else{b"AccountEntry:sarc -h 42.7 -p 98;"}).unwrap();
 let crate::connectivity::app_voice::providers::jegotrip::object_reference::ReferenceTarget::Direct(endpoints)=reference.target() else{panic!("endpoint")};
 let deadline=Instant::now()+Duration::from_secs(8);let peer=path.connect_peer(&original,&scope,&endpoints[0],0,deadline).await.unwrap().unwrap();
 let lease=path.listen(&original,&scope,ListenProtocol::Arc,Some(12000),false,deadline).await.unwrap().unwrap();
 assert!(path.send_datagram(&original,&scope,&lease,&peer,77,0,b"channel-frame-not-sdk-ready",deadline).await.unwrap());
 assert_eq!(path.ids(),Some((12001,12345)));drop(path);assert!(!scope.is_open());worker.await.unwrap();
}

#[tokio::test]
async fn original_tcp_tls_numeric_peer_negotiation_and_owned_data_keep_same_socket(){original_peer_flow(false).await;}

#[tokio::test]
async fn original_tcp_tls_logical_resolution_peer_negotiation_and_owned_data_keep_same_socket(){original_peer_flow(true).await;}

#[tokio::test]
async fn production_router_echo_and_delay_measurements_come_from_original_tls_headers(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);
 let worker=tokio::spawn(async move{tokio::time::timeout(Duration::from_secs(8),async move{
  let(tcp,_)=listener.accept().await.unwrap();let mut tls=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await.unwrap();let _connect=packet(&mut tls).await;
  let mut connected=reply();connected[6..8].copy_from_slice(&1000u16.to_be_bytes());tls.write_all(&connected).await.unwrap();tls.flush().await.unwrap();
  let probe=packet(&mut tls).await;assert_node_packet(&probe,0);
  let echoed=u16::from_be_bytes([probe[4],probe[5]]);assert!((1000..16000).contains(&echoed));assert_eq!(probe[6],0);
  let local=u16::from_be_bytes([probe[2],probe[3]]).max(1);let mut alive=node_wire(41,1);alive[8..10].copy_from_slice(&local.to_be_bytes());alive[10]=17;
  tls.write_all(&alive).await.unwrap();tls.flush().await.unwrap();let mut end=[0;1];assert!(matches!(tls.read(&mut end).await,Ok(0)|Err(_)));
 }).await.unwrap()});
 let original=owner();let scope=RpcConnectionScope::fresh();let mut path=RouterPath::open(original.clone(),scope.clone(),&p,&[],timeouts()).await.unwrap();
 assert_eq!(path.path_metrics(&original,&scope).unwrap().echoed_delay_ms,None);
 let _lease=path.listen(&original,&scope,ListenProtocol::Arc,None,false,Instant::now()+Duration::from_secs(3)).await.unwrap().unwrap();
 let measured=path.path_metrics(&original,&scope).unwrap();assert!(measured.echoed_delay_ms.is_some());assert_eq!(measured.peer_loss,Some(17));
 assert!(path.path_metrics(&owner(),&scope).is_none());drop(path);assert!(!scope.is_open());worker.await.unwrap();
}

// Test-only delayed adapter around the SAME original TCP owner. It neither
//creates a socket nor changes its scope/trust; exercised only in Actions.
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum DelayPoint{Write,Flush}
struct SlowOriginalIo{inner:BoxRuntimeIo,point:DelayPoint,delay:Duration,sleep:Option<std::pin::Pin<Box<tokio::time::Sleep>>>,waited:bool}
impl SlowOriginalIo{
 fn pause(&mut self,cx:&mut std::task::Context<'_>)->std::task::Poll<()>{
  use std::future::Future;
  if self.waited{return std::task::Poll::Ready(());}
  let delay=self.delay;let sleep=self.sleep.get_or_insert_with(||Box::pin(tokio::time::sleep(delay)));
  if sleep.as_mut().poll(cx).is_pending(){return std::task::Poll::Pending;}
  self.waited=true;self.sleep=None;std::task::Poll::Ready(())
 }
}
impl tokio::io::AsyncRead for SlowOriginalIo{
 fn poll_read(self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>,buffer:&mut tokio::io::ReadBuf<'_>)->std::task::Poll<std::io::Result<()>>{
  tokio::io::AsyncRead::poll_read(std::pin::Pin::new(&mut *self.get_mut().inner),cx,buffer)
 }
}
impl tokio::io::AsyncWrite for SlowOriginalIo{
 fn poll_write(self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>,bytes:&[u8])->std::task::Poll<std::io::Result<usize>>{
  let this=self.get_mut();if this.point==DelayPoint::Write&&this.pause(cx).is_pending(){return std::task::Poll::Pending;}
  let result=tokio::io::AsyncWrite::poll_write(std::pin::Pin::new(&mut *this.inner),cx,bytes);if this.point==DelayPoint::Write&&result.is_ready(){this.waited=false;}result
 }
 fn poll_flush(self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>)->std::task::Poll<std::io::Result<()>>{
  let this=self.get_mut();if this.point==DelayPoint::Flush&&this.pause(cx).is_pending(){return std::task::Poll::Pending;}
  let result=tokio::io::AsyncWrite::poll_flush(std::pin::Pin::new(&mut *this.inner),cx);if this.point==DelayPoint::Flush&&result.is_ready(){this.waited=false;}result
 }
 fn poll_shutdown(self:std::pin::Pin<&mut Self>,cx:&mut std::task::Context<'_>)->std::task::Poll<std::io::Result<()>>{
  tokio::io::AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut *self.get_mut().inner),cx)
 }
}
pub(crate) fn delay_original_io(path:&mut RouterPath,point:DelayPoint,delay:Duration){
 let inner=path.io.take().expect("original fixture socket");path.io=Some(Box::new(SlowOriginalIo{inner,point,delay,sleep:None,waited:false}));
}
