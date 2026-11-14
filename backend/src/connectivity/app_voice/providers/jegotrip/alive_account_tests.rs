//! Actions-only matched bootstrap -> independent Alive -> main ClientEx -> stop/Join.
use super::*;
use n::{account_runtime::AccountManager,account_test_support::TempDatabase,account_config::AccountConfig,transport_packets::CallKind};
use super::io::Peer;
async fn bounded<F:std::future::Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(12),future).await.expect("original account Alive fixture budget")}
fn config(root:&str,port:u16,alive_port:u16,approved:bool)->AccountConfig{
    let mut config=bootstrap_account_config(root,port);config.profile.incoming=None;config.profile.native_receivers=Some(NativeReceiverConfig{direct_domains:None});config.profile.client_ex=true;
    config.profile.account_alive=Some(policy_config(alive_port,approved));config
}
async fn check_reply(peer:&mut CommonPeer,id:i32)->Vec<u8>{loop{
    let bytes=peer.next().await.unwrap();if bytes[0]==1{let reply=decode_reply(&bytes[1..]).unwrap();assert_eq!(reply.call_id,id);return reply.result_stream.to_vec();}
    let request=decode_request(&bytes[1..]).unwrap();assert_eq!(request.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(request.arguments).unwrap();
    assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let data=n::rpc_batch::read_payload(&mut r).unwrap();
    peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,data.fields))]).await;
}}
async fn stop_main(peer:&mut CommonPeer){let(_,flag)=hosting_call(peer).await;assert_eq!(flag,b"3");loop{
    let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();
    if request.method==n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){peer.send(&[reply(request.call_id,&[0,1,0,0,0,0])]).await;break;}
    assert_eq!(request.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(request.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let body=n::rpc_batch::read_payload(&mut r).unwrap();
    peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,body.fields))]).await;
}while peer.next().await.is_some(){}}
async fn account_flow(approved:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let front=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=front.local_addr().unwrap().port();
    let source=TcpListener::bind("127.0.0.1:0").await.unwrap();let source_port=source.local_addr().unwrap().port();let destination=TcpListener::bind("127.0.0.1:0").await.unwrap();let destination_port=destination.local_addr().unwrap().port();
    let alive=TcpListener::bind("127.0.0.1:0").await.unwrap();let alive_port=alive.local_addr().unwrap().port();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {alive_port};");
    let tls=cert.server_config(&TLS13);let source_tls=tls.clone();let(early,after_early)=tokio::sync::oneshot::channel();let(registered,after_registered)=tokio::sync::oneshot::channel();let(eof,after_eof)=tokio::sync::oneshot::channel();let(ready,after_ready)=tokio::sync::oneshot::channel();
    let relay=tokio::spawn(async move{let mut tasks=Vec::new();for port in [source_port,destination_port]{let(mut input,_)=front.accept().await.unwrap();let mut output=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.unwrap();tasks.push(tokio::spawn(async move{let _=tokio::io::copy_bidirectional(&mut input,&mut output).await;}));}for task in tasks{task.await.unwrap();}});
    let bootstrap=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(source,source_tls).await;let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,n::rpc_bootstrap::GET_APP_CONFIGS_METHOD.as_bytes());
        peer.send(&[reply(request.call_id,&bootstrap_result_alive(reference.as_bytes()))]).await;while peer.next().await.is_some(){};}).await;});
    let alive_server=tokio::spawn(async move{bounded(async move{if approved{
        let mut peer=Peer::accept(alive).await;let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Request);let id=super::io::assert_registration(&bytes,b"fixture-user",b"fixture-original-selector",b"fixture-auth");
        assert_eq!(decode_request(&bytes).unwrap().metadata.iter().find(|p|p.key==b"mainAccount").unwrap().value,b"fixture-main-account");
        after_early.await.unwrap();peer.send(CallKind::Reply,&alive_reply(id,true,b"")).await;peer.send(CallKind::Request,&check_request(7)).await;
        let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Reply);let reply=decode_reply(&bytes).unwrap();let mut r=Reader::new(reply.result_stream).unwrap();assert!(r.boolean().unwrap());assert_eq!(r.u32().unwrap(),60);
        registered.send(()).unwrap();peer.eof().await;eof.send(()).unwrap();
    }else{
        after_early.await.unwrap();assert!(tokio::time::timeout(Duration::from_millis(30),alive.accept()).await.is_err());registered.send(()).unwrap();eof.send(()).unwrap();
    }}).await;});
    let main=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(destination,tls).await;let packet=peer.next().await.unwrap();let login=decode_request(&packet[1..]).unwrap();
        let mut check=vec![0];check.extend_from_slice(&check_request(11));peer.send(&[reply(login.call_id,&login_result_for_account()),check]).await;
        assert_eq!(check_reply(&mut peer,11).await,[0;10]);early.send(()).unwrap();let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");after_registered.await.unwrap();
        let mut check=vec![0];check.extend_from_slice(&check_request(12));peer.send(&[check]).await;
        let bytes=check_reply(&mut peer,12).await;let mut r=Reader::new(&bytes).unwrap();assert_eq!(r.boolean().unwrap(),approved);assert_eq!(r.u32().unwrap(),if approved{60}else{0});ready.send(()).unwrap();
        // This owning stop waits for the independent socket to be closed,
        // followed by real status3 and the original SDK logout/EOF.
        after_eof.await.unwrap();stop_main(&mut peer).await;
    }).await;});
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let created=manager.create(config(&cert.root,port,alive_port,approved),100).await.unwrap();let line=created.account.line_id;
    manager.start(&line,1,100).await.unwrap();bounded(after_ready).await.unwrap();let view=manager.get(&line,100).await.unwrap();assert_eq!(view.phase,"hosted");assert!(view.incoming_message_ready);assert!(!view.incoming_call_ready);
    assert_eq!(view.account_alive_state,Some(if approved{"registered"}else{"blocked"}));let serialized=serde_json::to_string(&view).unwrap();for value in ["fixture-auth","fixture-main-account","fixture-original-selector","127.0.0.1"]{assert!(!serialized.contains(value));}
    let stopped=bounded(manager.stop(&line,1,100)).await.unwrap();assert!(!stopped.io_cleanup_required);assert!(!stopped.incoming_message_ready);bounded(main).await.unwrap();bounded(alive_server).await.unwrap();bounded(bootstrap).await.unwrap();bounded(relay).await.unwrap();
    assert!(!temp.database.with_conn(|c|n::account_runs::ownership_pending(c,&line)).unwrap());
}
#[tokio::test]
async fn native_bootstrap_independent_alive_main_and_agent_checks_and_stop_join_keep_original_account(){Box::pin(account_flow(true)).await;}
#[tokio::test]
async fn native_bootstrap_without_legacy_approval_has_no_independent_io_and_no_positive_check(){Box::pin(account_flow(false)).await;}
#[test]
fn alive_profile_requires_current_native_bootstrap_client_ex_and_explicit_configuration(){
    let cert=Certificates::generate("IP:127.0.0.1");let config=config(&cert.root,12345,8888,false);config.validate(100).unwrap();let bytes=config.encode_private().unwrap();let loaded=AccountConfig::decode_private(&bytes).unwrap();assert_eq!(bytes.as_slice(),loaded.encode_private().unwrap().as_slice());
    let mut no_bootstrap=config.clone();no_bootstrap.profile.bootstrap_via_router=None;assert!(no_bootstrap.runtime_profile().is_err());
    let mut no_controls=config.clone();no_controls.profile.client_ex=false;assert!(no_controls.runtime_profile().is_err());
    let mut no_receiver=config;no_receiver.profile.native_receivers=None;assert!(no_receiver.runtime_profile().is_err());
}
