//! Actions-only connector/gate coverage; no system resolver modification or Provider.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::connect_failure_detail::ConnectFailureStep as Step;
use std::{sync::Arc,time::Instant};
async fn held_timeout(step:Step){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut p=policy(&cert.root,listener.local_addr().unwrap().port());let gate=p.connect_detail_test_gate(step);let who=owner();let scope=RpcConnectionScope::fresh();
    let connector=RouterTcpConnector::new(Duration::from_millis(60)).unwrap();let start=Instant::now();let future=connector.connect(&who,&scope,&p);tokio::pin!(future);
    let(waited,_)=tokio::join!(future.as_mut(),async{gate.entered().await;let(original,source)=gate.original_scope().unwrap();assert_eq!(original,who);assert!(source.matches(&scope));});
    assert_eq!(waited.err(),Some(ConnectError::Timeout));assert_eq!(scope.connect_timeout_for(&who),Some(step));assert!(start.elapsed()>=Duration::from_millis(60)&&start.elapsed()<Duration::from_secs(2));
    assert!(scope.is_open());assert_eq!(scope.native_rpc_resources(),(0,0));assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());scope.close();gate.release();
}
#[tokio::test]
async fn router_connect_detail_observes_original_dns_deadline_before_any_socket_poll(){held_timeout(Step::Dns).await;}
#[tokio::test]
async fn router_connect_detail_observes_original_tcp_deadline_without_any_tls_login_bytes(){held_timeout(Step::Tcp).await;}
#[tokio::test]
async fn router_connect_detail_real_loopback_success_never_fabricates_timeout(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=policy(&cert.root,listener.local_addr().unwrap().port());let who=owner();let scope=RpcConnectionScope::fresh();
    let connected=RouterTcpConnector::new(Duration::from_secs(1)).unwrap().connect(&who,&scope,&p).await.unwrap();let(mut peer,_)=listener.accept().await.unwrap();assert_eq!(scope.connect_timeout_for(&who),None);assert!(tokio::time::timeout(Duration::from_millis(20),peer.read(&mut[0;1])).await.is_err());
    let(index,io)=connected.into_original(&who,&scope,&p).unwrap();assert_eq!(index,0);drop(io);assert_eq!(peer.read(&mut[0;1]).await.unwrap(),0);scope.close();
}
#[tokio::test]
async fn router_connect_detail_closed_pending_original_scope_is_not_timeout_or_grant(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut p=policy(&cert.root,listener.local_addr().unwrap().port());let gate=p.connect_detail_test_gate(Step::Dns);let who=owner();let scope=RpcConnectionScope::fresh();
    let connector=RouterTcpConnector::new(Duration::from_secs(1)).unwrap();let future=connector.connect(&who,&scope,&p);tokio::pin!(future);let(result,_)=tokio::join!(future.as_mut(),async{gate.entered().await;scope.close();});
    assert_eq!(result.err(),Some(ConnectError::Scope));assert_eq!(scope.connect_timeout_for(&who),None);assert!(!scope.is_open());assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());gate.release();
}
#[tokio::test]
async fn router_connect_detail_cancelled_pending_future_does_not_record_generic_timeout(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut p=policy(&cert.root,listener.local_addr().unwrap().port());let gate=p.connect_detail_test_gate(Step::Tcp);let who=owner();let scope=RpcConnectionScope::fresh();let s=scope.clone();let w=who.clone();
    let task=tokio::spawn(async move{RouterTcpConnector::new(Duration::from_secs(1)).unwrap().connect(&w,&s,&p).await});gate.entered().await;task.abort();assert!(task.await.unwrap_err().is_cancelled());assert_eq!(scope.connect_timeout_for(&who),None);scope.close();gate.release();assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
}
#[tokio::test]
async fn router_connect_detail_released_fixture_hold_keeps_real_io_and_no_error(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut p=policy(&cert.root,listener.local_addr().unwrap().port());let gate=p.connect_detail_test_gate(Step::Dns);let who=owner();let scope=RpcConnectionScope::fresh();
    let connector=RouterTcpConnector::new(Duration::from_secs(1)).unwrap();let future=connector.connect(&who,&scope,&p);tokio::pin!(future);let(result,_)=tokio::join!(future.as_mut(),async{gate.entered().await;gate.release();});let connected=result.unwrap();let(mut peer,_)=listener.accept().await.unwrap();assert_eq!(scope.connect_timeout_for(&who),None);let(_,io)=connected.into_original(&who,&scope,&p).unwrap();drop(io);assert_eq!(peer.read(&mut[0;1]).await.unwrap(),0);scope.close();
}
#[tokio::test]
async fn router_connect_detail_policy_specific_hold_cannot_intercept_equal_new_policy(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let mut p=policy(&cert.root,port);let gate=p.connect_detail_test_gate(Step::Dns);let other=policy(&cert.root,port);let who=owner();let scope=RpcConnectionScope::fresh();
    let connected=RouterTcpConnector::new(Duration::from_secs(1)).unwrap().connect(&who,&scope,&other).await.unwrap();assert!(gate.original_scope().is_none());assert_eq!(scope.connect_timeout_for(&who),None);let(_,io)=connected.into_original(&who,&scope,&other).unwrap();drop(io);let(mut peer,_)=listener.accept().await.unwrap();assert_eq!(peer.read(&mut[0;1]).await.unwrap(),0);scope.close();
}
#[tokio::test]
async fn router_connect_detail_timeout_keeps_last_holder_accounting_through_original_close(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut p=policy(&cert.root,listener.local_addr().unwrap().port());let gate=p.connect_detail_test_gate(Step::Tcp);let who=owner();let scope=RpcConnectionScope::fresh();let held=scope.reserve_native_rpc(100).unwrap();let last=Arc::clone(&held);
    assert_eq!(RouterTcpConnector::new(Duration::from_millis(60)).unwrap().connect(&who,&scope,&p).await.err(),Some(ConnectError::Timeout));assert_eq!(scope.connect_timeout_for(&who),Some(Step::Tcp));assert_eq!(scope.native_rpc_resources(),(1,100));scope.close();drop(held);assert_eq!(scope.native_rpc_resources(),(1,100));drop(last);assert_eq!(scope.native_rpc_resources(),(0,0));gate.release();
}
