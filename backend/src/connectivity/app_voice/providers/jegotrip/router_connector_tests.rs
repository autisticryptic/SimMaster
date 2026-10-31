//! Actions-only loopback tests; no provider, device, or original SDK execution.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{session::HostingSession,tests::imported,tls_engine::tests::Certificates};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
fn owner()->AttemptOwner{let mut s=HostingSession::new("fixture-router".into());s.import(imported(),100).unwrap();s.begin_login(100).unwrap()}
fn anchor(root:&str,port:u16)->RouterAnchor<'_>{RouterAnchor{router_id:12001,host:"127.0.0.1",port,authority:"127.0.0.1",root,enable_sni:false}}
fn policy(root:&str,port:u16)->RouterPolicy{RouterPolicy::supplied(&format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"),&[anchor(root,port)]).unwrap()}
#[test]
fn all_router_endpoints_must_have_one_exact_trust_association_before_dns(){
 let cert=Certificates::generate("IP:127.0.0.1");let reference="Router:tls -h 127.0.0.1 -p 443 -ri 12001;tls -h 127.0.0.2 -p 444 -ri 12002;";
 assert!(RouterPolicy::supplied(reference,&[anchor(&cert.root,443)]).is_err());
 assert!(RouterPolicy::supplied("Router:tls -h 127.0.0.1 -p 443 -ri 12001;",&[anchor(&cert.root,443),anchor(&cert.root,443)]).is_err());
 let second=RouterAnchor{router_id:12002,host:"127.0.0.2",port:444,authority:"127.0.0.2",root:&cert.root,enable_sni:false};
 let p=RouterPolicy::supplied(reference,&[anchor(&cert.root,443),second]).unwrap();assert_eq!(p.len(),2);
 assert!(!format!("{p:?}").contains("127.0.0"));
}
#[test]
fn router_policy_refuses_cross_authority_transport_changes_and_extra_anchors(){
 let cert=Certificates::generate("IP:127.0.0.1");let reference="Router:tls -h 127.0.0.1 -p 443 -ri 12001;";
 for a in [RouterAnchor{authority:"other.invalid",..anchor(&cert.root,443)},anchor("AA==",443)] {assert!(RouterPolicy::supplied(reference,&[a]).is_err());}
 for bad in ["Other:tls -h 127.0.0.1 -p 443 -ri 12001;","Router:tcp -h 127.0.0.1 -p 443 -ri 12001;","Router:tls -h 127.0.0.1 -p 443;","Router@locator"] {
  assert!(RouterPolicy::supplied(bad,&[anchor(&cert.root,443)]).is_err());
 }
 assert!(RouterPolicy::supplied(reference,&[anchor(&cert.root,443),anchor(&cert.root,444)]).is_err());
}
#[tokio::test]
async fn router_connected_io_cannot_move_between_equal_but_independently_created_policies(){
 let cert=Certificates::generate("IP:127.0.0.1");let p=policy(&cert.root,443);let other=policy(&cert.root,443);let owner=owner();let scope=RpcConnectionScope::fresh();
 let(io,mut peer)=tokio::io::duplex(16);let connected=ConnectedRouterIo::original(&owner,&scope,&p,0,io).unwrap();
 assert_eq!(connected.into_original(&owner,&scope,&other).err(),Some(ConnectError::Scope));
 assert!(!scope.is_open());assert_eq!(peer.read(&mut[0;1]).await.unwrap(),0);
}
#[tokio::test]
async fn router_connect_rechecks_original_scope_before_first_and_each_pending_network_poll(){
 use std::sync::atomic::{AtomicUsize,Ordering};
 let scope=RpcConnectionScope::fresh();let polls=AtomicUsize::new(0);let future=poll_fn(|_|{polls.fetch_add(1,Ordering::SeqCst);Poll::<()>::Pending});
 let waiting=poll_original(&scope,future);tokio::pin!(waiting);
 assert!(tokio::time::timeout(Duration::from_millis(10),waiting.as_mut()).await.is_err());let prior=polls.load(Ordering::SeqCst);assert!(prior>0);
 scope.close();assert_eq!(waiting.await,Err(ConnectError::Scope));assert_eq!(polls.load(Ordering::SeqCst),prior);
 let scope=RpcConnectionScope::fresh();scope.close();let f=poll_fn(|_|{polls.fetch_add(1,Ordering::SeqCst);Poll::Ready(())});
 assert_eq!(poll_original(&scope,f).await,Err(ConnectError::Scope));assert_eq!(polls.load(Ordering::SeqCst),prior);
}
#[tokio::test]
async fn real_router_tcp_selection_returns_only_original_io_without_tls_or_login_bytes(){
 let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
 let p=policy(&cert.root,listener.local_addr().unwrap().port());let owner=owner();let scope=RpcConnectionScope::fresh();
 let connected=RouterTcpConnector::new(Duration::from_secs(2)).unwrap().connect(&owner,&scope,&p).await.unwrap();let(mut peer,_)=listener.accept().await.unwrap();
 assert!(tokio::time::timeout(Duration::from_millis(10),peer.read(&mut[0;1])).await.is_err());
 let(index,mut io)=connected.into_original(&owner,&scope,&p).unwrap();assert_eq!(p.router_id(index).unwrap(),12001);
 io.write_all(b"fixture").await.unwrap();let mut bytes=[0;7];peer.read_exact(&mut bytes).await.unwrap();assert_eq!(&bytes,b"fixture");drop(io);
}
#[test]
fn router_same_socket_different_ri_is_not_deduplicated_and_timeout_is_bounded(){
 let cert=Certificates::generate("IP:127.0.0.1");let reference="Router:tls -h 127.0.0.1 -p 443 -ri 12001;tls -h 127.0.0.1 -p 443 -ri 12002;";
 let p=RouterPolicy::supplied(reference,&[anchor(&cert.root,443),RouterAnchor{router_id:12002,..anchor(&cert.root,443)}]).unwrap();assert_eq!(p.len(),2);
 assert_eq!(p.router_id(0).unwrap(),12001);assert_eq!(p.router_id(1).unwrap(),12002);
 for timeout in [Duration::ZERO,Duration::from_secs(11)]{assert!(RouterTcpConnector::new(timeout).is_err());}
}
