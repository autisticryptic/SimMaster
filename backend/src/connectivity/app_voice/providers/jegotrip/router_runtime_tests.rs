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
