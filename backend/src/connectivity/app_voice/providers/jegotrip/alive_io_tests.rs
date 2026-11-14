//! Actions-only raw TCP/Common independent agent exchanges; synthetic tokens only.
use super::*;
use tokio::io::{AsyncReadExt,AsyncWriteExt};
use n::{netstream_records::{RecordSender,RecordReceiver,RecordSecurity},common_packets::CommonAssembler,transport_packets::CallKind};
pub(super) struct Peer{socket:tokio::net::TcpStream,tx:RecordSender,rx:RecordReceiver,assembler:CommonAssembler}
impl Peer{
    pub(super) async fn accept(listener:TcpListener)->Self{let(socket,_)=listener.accept().await.unwrap();Self{socket,tx:RecordSender::new(RecordSecurity::SeededXor,Some(0x12345678)).unwrap(),rx:RecordReceiver::new(RecordSecurity::SeededXor),assembler:CommonAssembler::new(Duration::from_secs(2),Instant::now()).unwrap()}}
    pub(super) async fn send(&mut self,kind:CallKind,payload:&[u8]){for packet in n::common_packets::encode(kind,payload,65536,None).unwrap(){let bytes=self.tx.encode(packet.expose_to_transport()).unwrap();self.socket.write_u32(bytes.expose_to_transport().len() as u32).await.unwrap();self.socket.write_all(bytes.expose_to_transport()).await.unwrap();}}
    pub(super) async fn next(&mut self)->(CallKind,Vec<u8>){loop{let length=self.socket.read_u32().await.unwrap()as usize;assert!((1..=131076).contains(&length));let mut bytes=vec![0;length];self.socket.read_exact(&mut bytes).await.unwrap();let plain=self.rx.decode(&bytes).unwrap();let now=Instant::now();if let Some(message)=self.assembler.on_packet(plain.expose_to_transport(),now+Duration::from_secs(2),now).unwrap(){return(message.kind,message.payload.expose_to_transport().to_vec());}}}
    pub(super) async fn eof(&mut self){let mut b=[0;64];assert!(matches!(self.socket.read(&mut b).await,Ok(0)|Err(_)));}
}
fn budgets()->AliveBudgets{AliveBudgets{connect:Duration::from_secs(2),rpc:Duration::from_secs(2),partial:Duration::from_millis(200),io:Duration::from_secs(1)}}
async fn wait<F:std::future::Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(5),future).await.expect("original independent Alive fixture budget")}
pub(super) fn assert_registration(bytes:&[u8],user:&[u8],selector:&[u8],token:&[u8])->i32{
    let request=decode_request(bytes).unwrap();assert_eq!(request.method,n::rpc_alive::REGISTER_ALIVE_METHOD.as_bytes());assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"AccountAlive");
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,token);assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));
    let args=n::rpc_alive::decode_register_alive_arguments(request.arguments).unwrap();assert_eq!(args.user_id,user);assert_eq!(args.selected_session_id,selector);assert_eq!(args.interval_seconds,60);assert!(args.request_timestamp_ms>1700000000000);request.call_id
}
#[tokio::test]
async fn original_independent_stcp_register_and_check_callbacks_use_actual_token_and_join(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let(done,checked)=tokio::sync::oneshot::channel();
    let server=tokio::spawn(async move{wait(async move{let mut peer=Peer::accept(listener).await;let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Request);
        let id=assert_registration(&bytes,b"fixture-alive-user",b"fixture-alive-selector",b"fixture-alive-token");assert_eq!(decode_request(&bytes).unwrap().metadata.iter().find(|p|p.key==b"mainAccount").unwrap().value,b"fixture-main-account");
        peer.send(CallKind::Request,&check_request(7)).await;let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Reply);assert_eq!(decode_reply(&bytes).unwrap().result_stream,[0;10]);
        peer.send(CallKind::Reply,&alive_reply(id,true,b"")).await;peer.send(CallKind::Request,&check_request(8)).await;
        let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Reply);let reply=decode_reply(&bytes).unwrap();let mut r=Reader::new(reply.result_stream).unwrap();assert!(r.boolean().unwrap());assert_eq!(r.u32().unwrap(),60);
        done.send(()).unwrap();peer.eof().await;}).await;});
    let(rpc,owner,scope,link)=authenticated();let cfg=AlivePolicy::capture(&policy_config(port,true)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");
    let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),budgets()).unwrap();wait(checked).await.unwrap();
    assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(true,60));let dispatcher=rpc.native_client_ex_dispatcher(receiver()).unwrap();let bytes=check_request(9);let request=decode_request(&bytes).unwrap();assert!(dispatcher.on_direct(&owner,&scope,&request).unwrap().unwrap().notice.is_none());
    wait(run.close()).await.unwrap();wait(server).await.unwrap();assert!(scope.is_open());assert_eq!(link.label(),"stopped");
}
#[tokio::test]
async fn unapproved_legacy_alive_configuration_never_opens_tcp_or_becomes_registered(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let(_rpc,owner,scope,link)=authenticated();
    let cfg=AlivePolicy::capture(&policy_config(port,false)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),budgets()).unwrap();
    assert_eq!(link.label(),"blocked");assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));assert!(tokio::time::timeout(Duration::from_millis(30),listener.accept()).await.is_err());run.close().await.unwrap();
}
#[tokio::test]
async fn stop_while_independent_registration_is_pending_closes_only_that_owned_io(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let(sent,seen)=tokio::sync::oneshot::channel();
    let server=tokio::spawn(async move{wait(async{let mut peer=Peer::accept(listener).await;peer.next().await;sent.send(()).unwrap();peer.eof().await;}).await;});
    let(_rpc,owner,scope,link)=authenticated();let cfg=AlivePolicy::capture(&policy_config(port,true)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),budgets()).unwrap();
    wait(seen).await.unwrap();wait(run.close()).await.unwrap();wait(server).await.unwrap();assert!(scope.is_open());assert!(!scope.session_revoked());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));
}
#[tokio::test]
async fn independent_alive_eof_drops_registration_and_waits_for_backoff_without_main_relogin(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let(finished,seen)=tokio::sync::oneshot::channel();
    let server=tokio::spawn(async move{wait(async{let mut peer=Peer::accept(listener).await;let(_,bytes)=peer.next().await;let id=decode_request(&bytes).unwrap().call_id;peer.send(CallKind::Reply,&alive_reply(id,true,b"")).await;drop(peer);finished.send(()).unwrap();}).await;});
    let(_rpc,owner,scope,link)=authenticated();let cfg=AlivePolicy::capture(&policy_config(port,true)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),budgets()).unwrap();wait(seen).await.unwrap();
    wait(async{while link.label()!="retrying"{tokio::task::yield_now().await;}}).await;assert!(scope.is_open());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));run.close().await.unwrap();wait(server).await.unwrap();
}
#[tokio::test]
async fn partial_stcp_prefix_does_not_renew_its_fixed_deadline(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let server=tokio::spawn(async move{wait(async{let mut peer=Peer::accept(listener).await;peer.next().await;
        for byte in [0,0,0,10]{if peer.socket.write_all(&[byte]).await.is_err(){break;}tokio::time::sleep(Duration::from_millis(70)).await;}peer.eof().await;}).await;});
    let(_rpc,owner,scope,link)=authenticated();let cfg=AlivePolicy::capture(&policy_config(port,true)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");let bounds=AliveBudgets{partial:Duration::from_millis(100),..budgets()};let start=Instant::now();let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),bounds).unwrap();
    wait(async{while link.label()!="retrying"{tokio::task::yield_now().await;}}).await;assert!(start.elapsed()<Duration::from_secs(1));assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));run.close().await.unwrap();wait(server).await.unwrap();
}
#[tokio::test]
async fn token_error_from_matching_alive_agent_revokes_the_main_scope_and_joins(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let server=tokio::spawn(async move{wait(async{let mut peer=Peer::accept(listener).await;let(_,bytes)=peer.next().await;let id=decode_request(&bytes).unwrap().call_id;peer.send(CallKind::Reply,&alive_reply(id,false,b"token-error:same-device-login")).await;peer.eof().await;}).await;});
    let(_rpc,owner,scope,link)=authenticated();let cfg=AlivePolicy::capture(&policy_config(port,true)).unwrap();let reference=format!("AccountAlive:stcp -h 127.0.0.1 -p {port};");let mut run=AliveRun::start(link.clone(),&owner,&scope,reference.as_bytes(),&cfg,receiver(),budgets()).unwrap();
    wait(async{while !scope.session_revoked(){tokio::task::yield_now().await;}}).await;run.close().await.unwrap();wait(server).await.unwrap();assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));
}
#[tokio::test]
async fn same_main_and_independent_scope_is_rejected_before_socket_poll_or_mutex_reentry(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let(_rpc,_owner,scope,_link)=authenticated();
    let endpoint=AccountAliveEndpoint{host:"127.0.0.1".into(),port};let now=Instant::now();
    assert_eq!(n::alive_io::AliveIo::connect(&endpoint,scope.clone(),scope.clone(),now+Duration::from_secs(1),budgets().partial,budgets().io).await.err(),Some("app_account_alive_io_failed"));
    assert!(scope.is_open());assert!(tokio::time::timeout(Duration::from_millis(30),listener.accept()).await.is_err());
}
#[tokio::test]
async fn independent_same_socket_refresh_preserves_cipher_and_rpc_ids_without_sdk_reauthentication(){
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();
    let server=tokio::spawn(async move{wait(async move{let mut peer=Peer::accept(listener).await;for expected in [1,2]{
        let(kind,bytes)=peer.next().await;assert_eq!(kind,CallKind::Request);let id=assert_registration(&bytes,b"fixture-alive-user",b"fixture-alive-selector",b"fixture-alive-token");assert_eq!(id,expected);
        peer.send(CallKind::Reply,&alive_reply(id,true,b"")).await;
    }peer.eof().await;}).await;});
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let agent_scope=agent.scope();
    let endpoint=AccountAliveEndpoint{host:"127.0.0.1".into(),port};let bounds=budgets();let mut io=wait(n::alive_io::AliveIo::connect(&endpoint,scope.clone(),agent_scope.clone(),Instant::now()+bounds.connect,bounds.partial,bounds.io)).await.unwrap();
    for timestamp in [1700000000124,1700000000125]{let now=Instant::now();let deadline=now+bounds.rpc;let request=agent.prepare(timestamp,now,deadline).unwrap();io.send(CallKind::Request,request.expose_to_transport(),deadline).await.unwrap();
        let calls=wait(async{loop{if let Some(calls)=io.receive(deadline).await.unwrap(){if !calls.is_empty(){break calls;}}}}).await;assert_eq!(calls.len(),1);
        assert!(matches!(agent.response(calls[0].payload.expose_to_transport(),Instant::now()).unwrap(),Some(AliveResult::Accepted)));assert!(agent.scope().matches(&agent_scope));
        assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(true,60));
    }
    drop(io);drop(agent);wait(server).await.unwrap();assert!(scope.is_open());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",Instant::now()),(false,0));
}
#[tokio::test]
async fn consumed_panicked_alive_join_is_not_repolled_on_cleanup_retry(){
    let(_rpc,_owner,scope,link)=authenticated();let task=tokio::spawn(async{panic!("synthetic original Alive task failure");});
    let mut run=AliveRun::with_task_for_test(link.clone(),task);assert_eq!(wait(run.close()).await.err(),Some("app_account_alive_cleanup_unconfirmed"));
    wait(run.close()).await.unwrap();assert!(scope.is_open());assert_eq!(link.label(),"stopped");
}
#[tokio::test]
async fn cancelled_alive_join_remains_owned_until_the_same_task_finishes_once(){
    let(_rpc,_owner,scope,link)=authenticated();let(resume,wait_resume)=tokio::sync::oneshot::channel();let finishes=Arc::new(std::sync::atomic::AtomicUsize::new(0));let counted=finishes.clone();
    let task=tokio::spawn(async move{wait_resume.await.unwrap();counted.fetch_add(1,std::sync::atomic::Ordering::SeqCst);});let mut run=AliveRun::with_task_for_test(link,task);
    assert!(tokio::time::timeout(Duration::from_millis(30),run.close()).await.is_err());assert_eq!(finishes.load(std::sync::atomic::Ordering::SeqCst),0);
    resume.send(()).unwrap();wait(run.close()).await.unwrap();wait(run.close()).await.unwrap();assert_eq!(finishes.load(std::sync::atomic::Ordering::SeqCst),1);assert!(scope.is_open());
}
