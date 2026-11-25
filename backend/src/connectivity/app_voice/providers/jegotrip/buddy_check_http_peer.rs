//! Actions-only authenticated HTTP -> current original checkRelation peer.
use super::*;
pub(crate) async fn serve_buddy_check(listener:TcpListener,server:Arc<ServerConfig>,mut finish:oneshot::Receiver<()>){
 tokio::time::timeout(Duration::from_secs(12),Box::pin(async move{let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();assert_eq!(login.method,rpc_login::LOGIN_SESSION_METHOD.as_bytes());let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-auth").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();peer.send(&[reply(login.call_id,w.finish().expose_to_transport())]).await;
 let mut known=SdkWrites::default();loop{let p=peer.next().await.unwrap();if swap(&mut peer,&p,Some(b"4"),&mut known).await{break;}}
 let id=loop{let p=peer.next().await.unwrap();let r=decode_request(&p[1..]).unwrap();if r.method==rpc_batch::SWAP_CALLS_METHOD.as_bytes(){assert!(!swap(&mut peer,&p,None,&mut known).await);continue;}assert_eq!(p[0],0);assert_eq!(r.method,b"checkRelation.GroupServer.Group");assert_eq!(r.metadata.len(),2);assert_eq!(r.metadata.iter().find(|v|v.key==b"id").unwrap().value,b"#Group");assert_eq!(r.metadata.iter().find(|v|v.key==b"token").unwrap().value,b"fixture-auth");let mut a=Reader::versioned(r.arguments).unwrap();assert_eq!(a.bytes().unwrap(),b"1_2");a.finish().unwrap();break r.call_id;};
 let mut w=Writer::new();w.boolean(true).unwrap();w.u32(u32::MAX).unwrap();w.bytes(b"").unwrap();w.properties(&[super::super::rpc_stream::RpcProperty{key:b"secret",value:b"not-a-grant"}]).unwrap();peer.send(&[reply(id,w.finish().expose_to_transport())]).await;
 loop{tokio::select!{biased;r=&mut finish=>{r.unwrap();break;},p=peer.next()=>{assert!(!swap(&mut peer,&p.unwrap(),None,&mut known).await);}}}
 loop{let p=peer.next().await.unwrap();if swap(&mut peer,&p,Some(b"3"),&mut known).await{break;}}
 loop{let p=peer.next().await.unwrap();let r=decode_request(&p[1..]).unwrap();if r.method==rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){peer.send(&[reply(r.call_id,&[0,1,0,0,0,0])]).await;break;}assert!(!swap(&mut peer,&p,None,&mut known).await);}
 while let Some(p)=peer.next_input_only().await{assert!(matches!(p.as_slice(),[2]|[14]|[4]),"no late Buddy check/retry/WRITE");}
 })).await.expect("original bounded HTTP peer");
}
