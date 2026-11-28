//! Actions-only original TCP/TLS/Router/Common peers. No service, SDK or device.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{arc_session::ArcReply,ue_alias_action::AliasAction,ue_alias_login::AliasLogin,
    runtime_provision::ProvisionedCommonLogin,router_runtime::tests::client_read_io::{ClientReadIoTest,ClientReadTestPoint}};
use std::{future::Future,sync::atomic::{AtomicUsize,Ordering}};
use tokio::sync::oneshot;

const ALIAS:&[u8]=b"private-alias";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Mode{Good,Empty,False,Bad,Version,DropSubmitted,Timeout,PolicyTimeout,GateRetired,AdmissionDrop,PrefixDrop,PrefixMixed,PrefixTimeout,PrefixClose,RejectedLogin,Unbound,WrongRouter}
impl Mode{
    fn held(self)->bool{matches!(self,Self::DropSubmitted|Self::Timeout|Self::PolicyTimeout|Self::GateRetired)}
    fn prefix(self)->bool{matches!(self,Self::PrefixDrop|Self::PrefixMixed|Self::PrefixTimeout|Self::PrefixClose)}
}
pub(super) async fn bounded<F:Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(12),future).await.expect("original alias fixture bound")}
fn aliases(mode:Mode)->Vec<u8>{
    let mut w=Writer::new();w.boolean(mode!=Mode::False).unwrap();let count=usize::from(mode!=Mode::Empty);w.u32(count as u32).unwrap();
    if count==1{w.bytes(b"username").unwrap();w.bytes(ALIAS).unwrap();w.boolean(true).unwrap();}w.bytes(b"").unwrap();
    let mut data=w.finish().expose_to_transport().to_vec();if mode==Mode::Bad{data.push(0xff);}data
}
fn bootstrap()->Vec<u8>{
    let i=login();let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&[
        RpcProperty{key:b"DomainName",value:i.domain_name},RpcProperty{key:b"DomainId",value:i.domain_id},
        RpcProperty{key:b"AppId",value:i.application_id},RpcProperty{key:b"CC",value:i.device_info_cc}]).unwrap();
    w.bytes(b"AccountEntry:sarc -h 42.7 -p 98;").unwrap();for _ in 0..3{w.bytes(b"").unwrap();}w.finish().expose_to_transport().to_vec()
}
fn swap_body(fields:[u32;6],results:&[(i32,u32,&[u8])],ack:bool)->Vec<u8>{
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,if ack{fields[0]}else{0},if ack{fields[1]}else{0},if ack{fields[2]}else{0}]{w.u32(word).unwrap();}
    w.u32(0).unwrap();w.u32(results.len()as u32).unwrap();for &(id,code,result)in results{w.i32(id).unwrap();w.u32(code).unwrap();w.bytes(result).unwrap();}
    w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn gateway()->GatewayRegistration<'static>{GatewayRegistration{authname_prefix:"fixture-prefix",channel_id:"fixture-channel",delete_int_code:"1",sip_number:"fixture-number",sip_password:"fixture-password"}}
async fn peer(listener:TcpListener,server:Arc<tokio_rustls::rustls::ServerConfig>,mode:Mode,seen:oneshot::Sender<()>,release:oneshot::Receiver<()>,reads:Arc<AtomicUsize>){
    let mut p=CommonPeer::open(listener,server).await;let packet=p.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
    assert_eq!(call.call_id,1);assert_eq!(call.method,b"loginSession.AccountEntry.Account");assert_eq!(call.metadata,[RpcProperty{key:b"id",value:b"AccountEntry"},RpcProperty{key:b"pwd",value:login().phone_login_token}]);
    p.send(&[reply(call.call_id,&login_result(mode!=Mode::RejectedLogin))]).await;
    if mode==Mode::RejectedLogin{p.no_more_common().await;return;}
    let mut seen=Some(seen);let mut release=Some(release);let mut registered=None;let mut last=1;let mut original_read=None;let mut held_reply=None;
    loop{
        let packet=if mode==Mode::PrefixClose{p.next_input_only().await}else{p.next().await};let Some(packet)=packet else{assert_eq!(mode,Mode::PrefixClose);assert!(original_read.is_none());return;};assert_eq!(packet[0],0);
        let call=decode_request(&packet[1..]).unwrap();assert!(call.call_id>last);last=call.call_id;assert_eq!(call.metadata.len(),2);assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");assert_eq!(call.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
        let mut r=Reader::versioned(call.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),login().selected_session_id);
        if call.method==n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){r.finish().unwrap();assert!(held_reply.is_none());assert_eq!(reads.load(Ordering::SeqCst),usize::from(!matches!(mode,Mode::AdmissionDrop|Mode::Unbound|Mode::WrongRouter)));p.send(&[reply(call.call_id,b"\0\x01\0\0\0\0")]).await;p.no_more_common().await;return;}
        assert_eq!(call.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let batch=n::rpc_batch::read_payload(&mut r).unwrap();r.finish().unwrap();assert!(batch.results.is_empty());let mut alias=None;let mut host=None;
        for(&seq,nested)in &batch.requests{
            if nested.method==b"getRelations.User.User"{assert!(original_read.is_none());assert_eq!(nested.metadata,[RpcProperty{key:b"id",value:b"#User"}]);assert_eq!(nested.arguments,[0,0,1,0,0]);assert_eq!(seq,if mode==Mode::PrefixMixed{2}else{1});alias=Some(seq);original_read=Some(seq);reads.fetch_add(1,Ordering::SeqCst);}
            else{assert_eq!(mode,Mode::PrefixMixed);assert_eq!(nested.method,n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());assert_eq!(seq,1);assert_eq!(nested.metadata,[RpcProperty{key:b"id",value:b"#SipGateway"}]);
                let mut args=Reader::versioned(nested.arguments).unwrap();assert_eq!(args.bytes().unwrap(),b"fixture-uri");let props=args.properties().unwrap();args.finish().unwrap();assert_eq!(props.iter().find(|p|p.key==b"SipFlag").unwrap().value,b"4");
                if let Some(prior)=&registered{assert_eq!(prior,nested.arguments);}else{registered=Some(nested.arguments.to_vec());host=Some(seq);}
            }
        }
        if let Some(seq)=alias{
            assert_eq!(batch.requests.len(),if mode==Mode::PrefixMixed{2}else{1});seen.take().unwrap().send(()).unwrap();
            if mode.held(){held_reply=Some((call.call_id,batch.fields,seq));release.take().unwrap().await.unwrap();continue;}
            let bytes=aliases(mode);let code=if mode==Mode::Version{65537}else{0};p.send(&[reply(call.call_id,&swap_body(batch.fields,&[(seq,code,&bytes)],true))]).await;
        }else if let Some((outer,fields,seq))=held_reply.take(){assert!(batch.requests.is_empty());let data=aliases(Mode::Good);p.send(&[reply(outer,&swap_body(fields,&[(seq,0,&data)],true)),reply(call.call_id,&swap_body(batch.fields,&[],true))]).await;}
        else{let result=host.map(|seq|vec![(seq,0,b"\0\x01\0\0\0\0".as_slice())]).unwrap_or_default();p.send(&[reply(call.call_id,&swap_body(batch.fields,&result,mode!=Mode::PrefixMixed))]).await;}
    }
}
pub(super) struct Fixture{pub(super) handle:RuntimeHandle,pub(super) events:mpsc::Receiver<RuntimeEvent>,pub(super) owner:AttemptOwner,pub(super) scope:RpcConnectionScope,pub(super) gate:Arc<ImReceiptGate>,pub(super) seen:oneshot::Receiver<()>,pub(super) release:Option<oneshot::Sender<()>>,done:Option<oneshot::Sender<()>>,sdk:Option<tokio::task::JoinHandle<()>>,relay:Option<tokio::task::JoinHandle<()>>,reads:Arc<AtomicUsize>,connections:Arc<AtomicUsize>}
impl Fixture{
    pub(super) async fn open(mode:Mode)->Self{
        let cert=Certificates::generate("IP:127.0.0.1");let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=front.local_addr().unwrap().port();let boot=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let main=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let ports=[boot.local_addr().unwrap().port(),main.local_addr().unwrap().port()];let server=cert.server_config(&TLS13);let bootserver=server.clone();
        let(done,finished)=oneshot::channel();let connections=Arc::new(AtomicUsize::new(0));let counted=connections.clone();
        let relay=tokio::spawn(async move{let mut joins=Vec::new();for port in ports{let(mut incoming,_)=front.accept().await.unwrap();counted.fetch_add(1,Ordering::SeqCst);let mut target=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.unwrap();joins.push(tokio::spawn(async move{let _=tokio::io::copy_bidirectional(&mut incoming,&mut target).await;}));}
            tokio::select!{r=front.accept()=>panic!("third alias socket: {}",r.is_ok()),r=finished=>r.unwrap()};for join in joins{bounded(join).await.unwrap();}});
        let source=tokio::spawn(async move{let mut p=CommonPeer::open(boot,bootserver).await;let bytes=p.next().await.unwrap();let call=decode_request(&bytes[1..]).unwrap();assert_eq!(call.method,b"getAppConfigs.AccountEntry.Account");p.send(&[reply(call.call_id,&bootstrap())]).await;p.no_more_common().await;});
        let(seen,seen_rx)=oneshot::channel();let(release,release_rx)=oneshot::channel();let reads=Arc::new(AtomicUsize::new(0));let sdk=tokio::spawn(Box::pin(peer(main,server,mode,seen,release_rx,reads.clone())));
        let owner=owner();let policy=router_policy(&cert.root,port);let destination=prepare_common(owner.clone(),rpc_policy(),Instant::now()+Duration::from_secs(6)).unwrap();let scope=destination.connection_scope();let original=reference(false);let ReferenceTarget::Direct(endpoints)=original.target()else{panic!()};
        let prepared=prepare_common(owner.clone(),rpc_policy(),Instant::now()+Duration::from_secs(6)).unwrap();let boot_scope=prepared.connection_scope();let( mut bh,mut be)=Box::pin(prepared.connect(&policy,&[],b"BootEntry",&endpoints[0],0,timeouts(),None)).await.unwrap().attach().unwrap();
        bounded(bh.bootstrap(&owner,&boot_scope,b"fixture-app-key",&[])).await.unwrap();let event=bounded(be.recv()).await.unwrap();let matched=event.into_events(&owner,&boot_scope).unwrap().into_iter().find_map(|e|if let OfflineEvent::Bootstrap(p)=e{Some(p)}else{None}).unwrap();
        let plan=ProvisionedCommonLogin::from_reply(&matched,&owner,&boot_scope,&scope,&policy,None,None).unwrap();assert!(bounded(bh.close()).await.io_released());bounded(source).await.unwrap();assert!(!boot_scope.is_open());
        let mut connected=Box::pin(destination.connect(&policy,&[],original.identity(),&endpoints[0],0,timeouts(),None)).await.unwrap();
        if mode==Mode::PolicyTimeout{connected.policy.as_mut().unwrap().rpc_timeout=Duration::from_millis(100);}
        let(handle,mut events)=connected.attach().unwrap();let command=handle.command_handle();let gate=ImReceiptGate::fresh();
        if mode==Mode::WrongRouter{let other=router_policy(&cert.root,port);let badplan=synthetic_plan(&owner,&scope,&other);let binding=AliasLogin::capture(&owner,&scope,&badplan,&other,gate.clone(),&login()).unwrap();assert_eq!(bounded(command.login_with_alias_source(&owner,&scope,binding,login(),&[])).await.err(),Some(RuntimeError::Protocol));assert_eq!(scope.native_rpc_resources(),(0,0));}
        let sent=if matches!(mode,Mode::Unbound|Mode::WrongRouter){bounded(command.login(&owner,&scope,login(),&[])).await.unwrap()}
            else{let binding=AliasLogin::capture(&owner,&scope,&plan,&policy,gate.clone(),&login()).unwrap();bounded(command.login_with_alias_source(&owner,&scope,binding,login(),&[])).await.unwrap()};assert_eq!(sent.call_id,Some(1));
        let event=bounded(events.recv()).await.unwrap();if mode==Mode::RejectedLogin{assert!(event.terminal_failure(&owner,&scope).is_some());}else{assert!(matches!(&event.events_for(&owner,&scope).unwrap()[0],OfflineEvent::Callback(c)if c.callback==Callback::SdkLogin{success:true}));bounded(command.configure(&owner,&scope,77,None)).await.unwrap();}
        Self{handle,events,owner,scope,gate,seen:seen_rx,release:Some(release),done:Some(done),sdk:Some(sdk),relay:Some(relay),reads,connections}
    }
    pub(super) fn action(&self,ms:u64)->Arc<AliasAction>{AliasAction::capture(&self.owner,&self.scope,self.gate.clone(),Instant::now()+Duration::from_millis(ms)).unwrap()}
    pub(super) async fn close(mut self,logout:bool){
        let released=if logout{bounded(self.handle.logout(&self.owner,&self.scope,&[])).await.unwrap()}else{bounded(self.handle.close()).await};assert!(released.io_released());assert!(!self.scope.is_open());
        self.done.take().unwrap().send(()).unwrap();bounded(self.sdk.take().unwrap()).await.unwrap();bounded(self.relay.take().unwrap()).await.unwrap();assert_eq!(self.connections.load(Ordering::SeqCst),2);
    }
}
fn synthetic_plan(owner:&AttemptOwner,dest:&RpcConnectionScope,policy:&RouterPolicy)->ProvisionedCommonLogin{
    let mut source=n::arc_session::ArcRpcSession::new(owner.clone());let scope=source.connection_scope();let(id,_)=source.prepare_bootstrap(b"fixture",&[]).unwrap();let bytes=reply(id,&bootstrap());let Some(ArcReply::Bootstrap(p))=source.on_reply(owner,&scope,&bytes[1..]).unwrap()else{panic!()};ProvisionedCommonLogin::from_reply(&p,owner,&scope,dest,policy,None,None).unwrap()
}
async fn result_case(mode:Mode){
    let f=Box::pin(Fixture::open(mode)).await;let command=f.handle.command_handle();
    if mode==Mode::RejectedLogin{f.close(false).await;return;}
    let action=f.action(3000);let result=bounded(command.read_aliases(action.clone(),&[])).await;
    if matches!(mode,Mode::Good|Mode::Empty){let result=result.unwrap();assert_eq!(result.pair(&f.owner,&f.scope,b"username",ALIAS),Some(mode==Mode::Good));assert_eq!(result.pair(&owner(),&f.scope,b"username",ALIAS),None);assert!(!format!("{result:?}").contains("private"));let scope=f.scope.clone();let charged=scope.native_rpc_resources();assert!(charged.1>0);f.close(true).await;assert!(!result.current());drop(action);drop(result);assert_eq!(scope.native_rpc_resources(),(0,0));}
    else{assert_eq!(result.err(),Some(if matches!(mode,Mode::Unbound|Mode::WrongRouter){RuntimeError::Scope}else{RuntimeError::Protocol}));assert!(f.scope.is_open());drop(action);f.close(true).await;}
}
#[tokio::test]async fn ue_alias_actor_real_bootstrap_same_router_login_and_result_join_all(){Box::pin(result_case(Mode::Good)).await;}
#[tokio::test]async fn ue_alias_actor_full_empty_is_observation_not_unknown_or_uid(){Box::pin(result_case(Mode::Empty)).await;}
#[tokio::test]async fn ue_alias_actor_false_result_is_not_success_and_sdk_logout_healthy(){Box::pin(result_case(Mode::False)).await;}
#[tokio::test]async fn ue_alias_actor_bad_footer_does_not_install_and_original_sdk_joins(){Box::pin(result_case(Mode::Bad)).await;}
#[tokio::test]async fn ue_alias_actor_version_failure_is_single_attempt_without_business_retry(){Box::pin(result_case(Mode::Version)).await;}
#[tokio::test]async fn ue_alias_actor_default_login_does_not_enable_alias_read(){Box::pin(result_case(Mode::Unbound)).await;}
#[tokio::test]async fn ue_alias_actor_equal_looking_foreign_router_rejected_before_native_id(){Box::pin(result_case(Mode::WrongRouter)).await;}
#[tokio::test]async fn ue_alias_actor_rejected_login_has_no_alias_receipt_and_all_joins(){Box::pin(result_case(Mode::RejectedLogin)).await;}

async fn held_case(mode:Mode){
    let mut f=Box::pin(Fixture::open(mode)).await;let c=f.handle.command_handle();let action=f.action(if mode==Mode::Timeout{100}else{3000});let mut reading=Box::pin(c.read_aliases(action.clone(),&[]));
    tokio::select!{r=reading.as_mut()=>panic!("reply held: {r:?}"),r=bounded(&mut f.seen)=>r.unwrap()}
    if matches!(mode,Mode::Timeout|Mode::PolicyTimeout){assert_eq!(bounded(reading).await.err(),Some(RuntimeError::Timeout));if mode==Mode::PolicyTimeout{assert!(Instant::now()<action.deadline());}}
    else if mode==Mode::GateRetired{f.gate.retire();assert_eq!(bounded(reading).await.err(),Some(RuntimeError::Cancelled));}
    else{drop(reading);}
    assert_eq!(bounded(c.read_aliases(action.clone(),&[])).await.err(),Some(RuntimeError::Scope));
    bounded(c.original_user_entry_auth(&f.owner,&f.scope)).await.unwrap();f.release.take().unwrap().send(()).unwrap();bounded(c.poll(&f.owner,&f.scope,&[])).await.unwrap();
    assert!(f.scope.is_open());assert_eq!(f.reads.load(Ordering::SeqCst),1);let scope=f.scope.clone();f.close(true).await;drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[tokio::test]async fn ue_alias_actor_submitted_cancel_retires_only_read_and_ignores_late_reply(){Box::pin(held_case(Mode::DropSubmitted)).await;}
#[tokio::test]async fn ue_alias_actor_timeout_is_not_new_sdk_failure_and_late_reply_cannot_install(){Box::pin(held_case(Mode::Timeout)).await;}
#[tokio::test]async fn ue_alias_actor_actual_shorter_rpc_deadline_cannot_borrow_preparation_budget(){Box::pin(held_case(Mode::PolicyTimeout)).await;}
#[tokio::test]async fn ue_alias_actor_parent_gate_retires_before_late_result_without_new_grant(){Box::pin(held_case(Mode::GateRetired)).await;}

async fn gated_case(mode:Mode){
    let mut f=Box::pin(Fixture::open(mode)).await;let c=f.handle.command_handle();
    if mode==Mode::PrefixMixed{bounded(c.hosting(&f.owner,&f.scope,"fixture-uri",STATUS_REGISTER,gateway(),&[RpcProperty{key:b"id",value:b"#SipGateway"}],&[])).await.unwrap();let event=bounded(f.events.recv()).await.unwrap();assert!(event.events_for(&f.owner,&f.scope).unwrap().iter().any(|e|matches!(e,OfflineEvent::Callback(c)if c.callback==Callback::HostingAck{success:true})));}
    let action=f.action(if mode==Mode::PrefixTimeout{100}else{3000});let test=ClientReadIoTest::original(if mode==Mode::AdmissionDrop{ClientReadTestPoint::Admission}else{ClientReadTestPoint::Prefix(1)});action.set_test(test.clone());let mut reading=Box::pin(c.read_aliases(action.clone(),&[]));
    tokio::select!{r=reading.as_mut()=>panic!("before original barrier: {r:?}"),_=bounded(async{if mode==Mode::AdmissionDrop{test.entered().await}else{test.paused().await}})=>{}}
    assert_eq!(test.sequence(),if mode==Mode::PrefixMixed{2}else{1});assert_eq!(f.reads.load(Ordering::SeqCst),0);assert_eq!(test.accepted.load(Ordering::SeqCst),usize::from(mode.prefix()));
    if mode==Mode::PrefixClose{f.scope.close();assert!(bounded(reading).await.is_err());let scope=f.scope.clone();f.close(false).await;assert!(test.dropped.load(Ordering::SeqCst));assert_eq!(test.accepted.load(Ordering::SeqCst),1);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));return;}
    if mode==Mode::PrefixTimeout{assert_eq!(bounded(reading).await.err(),Some(RuntimeError::Timeout));}else{drop(reading);}
    assert!(f.scope.is_open());assert!(f.scope.native_rpc_resources().1>0);test.release();bounded(c.original_user_entry_auth(&f.owner,&f.scope)).await.unwrap();
    if mode!=Mode::AdmissionDrop{bounded(&mut f.seen).await.unwrap();assert!(test.accepted.load(Ordering::SeqCst)>1);}
    let scope=f.scope.clone();f.close(true).await;if mode.prefix(){assert!(test.dropped.load(Ordering::SeqCst));}drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[tokio::test]async fn ue_alias_actor_cancel_before_encoding_consumes_no_frame_and_preserves_joins(){Box::pin(gated_case(Mode::AdmissionDrop)).await;}
#[tokio::test]async fn ue_alias_actor_original_tcp_prefix_cancel_finishes_frame_and_preserves_logout(){Box::pin(gated_case(Mode::PrefixDrop)).await;}
#[tokio::test]async fn ue_alias_actor_original_tcp_prefix_mixed_cancel_preserves_hosting_epoch(){Box::pin(gated_case(Mode::PrefixMixed)).await;}
#[tokio::test]async fn ue_alias_actor_original_tcp_prefix_deadline_does_not_abort_owned_frame(){Box::pin(gated_case(Mode::PrefixTimeout)).await;}
#[tokio::test]async fn ue_alias_actor_original_tcp_prefix_scope_close_stops_next_byte_and_joins(){Box::pin(gated_case(Mode::PrefixClose)).await;}

#[tokio::test]
async fn ue_alias_actor_shared_capacity_refuses_before_enqueue_and_refunds_last_action(){Box::pin(async{
    let f=Box::pin(Fixture::open(Mode::Good)).await;let c=f.handle.command_handle();let baseline=f.scope.native_rpc_resources();
    let mut held=Vec::new();while let Ok(charge)=f.scope.reserve_native_rpc(1){held.push(charge);}assert_eq!(f.scope.native_rpc_resources().0,128);
    assert_eq!(AliasAction::capture(&f.owner,&f.scope,f.gate.clone(),Instant::now()+Duration::from_secs(1)).err().unwrap(),n::rpc_stream::CodecError::TooLarge);drop(held);
    let action=f.action(3000);let retained=action.clone();let full=f.scope.reserve_native_rpc(128*1024-f.scope.native_rpc_resources().1).unwrap();
    assert_eq!(bounded(c.read_aliases(action.clone(),&[])).await.err(),Some(RuntimeError::Backpressure));assert_eq!(f.reads.load(Ordering::SeqCst),0);drop(full);drop(action);assert!(f.scope.native_rpc_resources().0>baseline.0);drop(retained);assert_eq!(f.scope.native_rpc_resources(),baseline);
    let foreign=AliasAction::capture(&owner(),&f.scope,f.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(bounded(c.read_aliases(foreign,&[])).await.err(),Some(RuntimeError::Scope));
    let own=f.action(3000);let reply=bounded(c.read_aliases(own.clone(),&[])).await.unwrap();assert_eq!(reply.pair(&f.owner,&f.scope,b"username",ALIAS),Some(true));drop(reply);drop(own);f.close(true).await;
}).await;}

#[tokio::test]
async fn ue_alias_actor_cancelled_close_keeps_original_join_and_stops_partial_frame(){Box::pin(async{
    let mut f=Box::pin(Fixture::open(Mode::PrefixClose)).await;let c=f.handle.command_handle();let action=f.action(3000);let test=ClientReadIoTest::original(ClientReadTestPoint::Prefix(1));action.set_test(test.clone());let mut reading=Box::pin(c.read_aliases(action.clone(),&[]));
    tokio::select!{r=reading.as_mut()=>panic!("before physical prefix: {r:?}"),_=bounded(test.paused())=>{}}drop(reading);
    {let mut cleanup=Box::pin(f.handle.close());std::future::poll_fn(|cx|{assert!(cleanup.as_mut().poll(cx).is_pending());std::task::Poll::Ready(())}).await;}
    assert!(f.handle.task.is_some());assert!(!f.scope.is_open());let scope=f.scope.clone();f.close(false).await;assert!(test.dropped.load(Ordering::SeqCst));assert_eq!(test.accepted.load(Ordering::SeqCst),1);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
}).await;}
