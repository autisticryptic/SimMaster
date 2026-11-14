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
