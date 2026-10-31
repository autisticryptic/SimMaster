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
