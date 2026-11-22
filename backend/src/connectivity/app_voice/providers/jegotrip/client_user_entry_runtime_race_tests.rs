//! Actions-only actual original actor admission and main TCP prefix regressions.
//! Separate strict peer/helper: no original case/assertion/deadline is weakened.
use super::*;
use n::router_runtime::tests::client_read_io::{ClientReadIoTest, ClientReadTestPoint};
use std::future::Future;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Race { AdmissionEmpty, AdmissionMixed, PrefixDrop, PrefixMixed, PrefixTimeout, PrefixStop, PrefixClose, CleanupCancelled, SharedBudget, ReplyHolder }
impl Race {
    fn mixed(self)->bool{matches!(self,Self::AdmissionMixed|Self::PrefixMixed)}
    fn admission(self)->bool{matches!(self,Self::AdmissionEmpty|Self::AdmissionMixed)}
    fn fatal(self)->bool{matches!(self,Self::PrefixClose|Self::CleanupCancelled)}
}
fn status(nested:&n::rpc_batch::BatchRequest<'_>,uri:&[u8])->u8{
    assert_eq!(nested.method,n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
    assert_eq!(nested.metadata.iter().find(|p|p.key==b"id").unwrap().value,n::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes());
    let mut r=Reader::versioned(nested.arguments).unwrap();assert_eq!(r.bytes().unwrap(),uri);let fields=r.properties().unwrap();r.finish().unwrap();
    assert_eq!(fields.iter().find(|p|p.key==b"SipPwd").unwrap().value,b"fixture-SipPwd");
    match fields.iter().find(|p|p.key==b"SipFlag").unwrap().value{b"4"=>4,b"3"=>3,_=>panic!("original hosting flag")}
}
fn refresh_reply()->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(0).unwrap();w.i64(1).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
async fn strict_peer(listener:TcpListener,config:Arc<tokio_rustls::rustls::ServerConfig>,race:Race,captured:LoginCapture,uri:Vec<u8>,observed:oneshot::Sender<()>,reads:Arc<AtomicUsize>,unhosts:Arc<AtomicUsize>){
    let mut peer=CommonPeer::open(listener,config).await;let bytes=peer.next().await.unwrap();assert_eq!(bytes[0],0);
    let login=decode_request(&bytes[1..]).unwrap();assert_eq!(login.method,b"loginSession.AccountEntry.Account");
    assert_eq!(login.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");
    assert_eq!(login.metadata.iter().find(|p|p.key==b"pwd").unwrap().value,captured.password.as_bytes());assert!(!login.metadata.iter().any(|p|p.key==b"token"));
    let mut arguments=Reader::versioned(login.arguments).unwrap();assert_eq!(arguments.bytes().unwrap(),uri);assert_eq!(arguments.bytes().unwrap(),captured.selector.as_bytes());let properties=arguments.properties().unwrap();arguments.finish().unwrap();
    for(key,value)in [(b"__domain".as_slice(),b"fixture-domain".as_slice()),(b"__app",b"fixture-app"),(b"DeviceInfo.CC",b"fixture-cc"),(b"__appKey",captured.appkey.as_bytes())]{assert_eq!(properties.iter().find(|p|p.key==key).unwrap().value,value);}
    if let Some(device)=&captured.device{assert_eq!(properties.iter().find(|p|p.key==b"DeviceId").unwrap().value,device.as_bytes());}
    peer.send(&[reply(login.call_id,&login_result())]).await;
    let(mut registered,mut unregistered,mut read_seen,mut admission_seen)=(false,false,false,false);let mut observed=Some(observed);let mut group_reads=0;
    loop{
        let next=if registered&&race.fatal(){peer.next_input_only().await}else{peer.next().await};
        let Some(bytes)=next else{assert!(race.fatal());assert!(registered&&!unregistered&&!read_seen);assert_eq!(group_reads,0);return;};assert_eq!(bytes[0],0);
        let call=decode_request(&bytes[1..]).unwrap();assert_eq!(call.metadata.len(),2);
        assert_eq!(call.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert!(!call.metadata.iter().any(|p|p.key==b"pwd"));
        if call.method==n::im_status::REFRESH_METHOD.as_bytes(){
            assert_eq!(race,Race::SharedBudget);assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert!(registered&&!read_seen&&!unregistered);
            group_reads+=1;assert!(group_reads<=4,"Client input pressure must emit no unrelated Group frame");peer.send(&[reply(call.call_id,&refresh_reply())]).await;continue;
        }
        assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");
        let mut arguments=Reader::versioned(call.arguments).unwrap();assert_eq!(arguments.bytes().unwrap(),b"fixture-user");assert_eq!(arguments.bytes().unwrap(),captured.selector.as_bytes());
        if call.method==b"logoutSession.AccountEntry.Account"{
            arguments.finish().unwrap();assert!(registered&&unregistered&&!race.fatal());assert_eq!(read_seen,!race.admission());
            assert_eq!(group_reads,if race==Race::SharedBudget{4}else{0});peer.send(&[reply(call.call_id,b"\0\x01\0\0\0\0")]).await;peer.no_more_common().await;return;
        }
        assert_eq!(call.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let payload=n::rpc_batch::read_payload(&mut arguments).unwrap();arguments.finish().unwrap();assert!(payload.results.is_empty());
        let mut read_sequence=None;let mut status_sequence=None;let was_registered=registered;let mut retained_register=false;
        for(&sequence,nested)in &payload.requests{
            if nested.method==n::user_entry_data::AID_TO_UID_METHOD.as_bytes(){
                assert!(!race.admission()&&!race.fatal()&&!read_seen&&registered&&!unregistered);assert_eq!(sequence,2);assert_eq!(nested.metadata.len(),2);
                assert_eq!(nested.metadata.iter().find(|p|p.key==b"id").unwrap().value,FACTORY);assert_eq!(nested.metadata.iter().find(|p|p.key==b"DeviceInfo.CC").unwrap().value,b"fixture-cc");
                assert!(!nested.metadata.iter().any(|p|matches!(p.key,b"token"|b"pwd"|b"accountid")));let mut r=Reader::versioned(nested.arguments).unwrap();assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"phone");assert_eq!(r.bytes().unwrap(),b"+12345");r.finish().unwrap();
                read_sequence=Some(sequence);read_seen=true;reads.fetch_add(1,Ordering::SeqCst);
            }else{
                match status(nested,&uri){4=>{assert_eq!(sequence,1);if registered{assert!(race.mixed());retained_register=true;}else{registered=true;status_sequence=Some(sequence);}},3=>{assert!(registered&&!race.fatal());assert_eq!(sequence,3,"actual admitted READ sequence2 must be burned even when never encoded");unregistered=true;status_sequence=Some(sequence);unhosts.fetch_add(1,Ordering::SeqCst);},_=>unreachable!()}
            }
        }
        if let Some(sequence)=read_sequence{
            assert_eq!(retained_register,race.mixed());let result=answer(Case::Reported);peer.send(&[reply(call.call_id,&swap_answer(payload.fields,&[(sequence,0,&result)],!race.mixed()))]).await;
            observed.take().unwrap().send(()).unwrap();
        }else if was_registered&&!unregistered&&!admission_seen&&race.admission(){
            admission_seen=true;assert_eq!(retained_register,race.mixed());assert_eq!(payload.requests.len(),usize::from(race.mixed()));observed.take().unwrap().send(()).unwrap();
            if race.mixed(){peer.send(&[reply(call.call_id,&swap_answer(payload.fields,&[],false))]).await;}
            // Empty READ-only outer gets NO reply. Healthy unregister/logout
            // must not inherit its fatal pending deadline or require its ACK.
        }else{
            if unregistered&&race.mixed(){assert!(retained_register);assert_eq!(payload.requests.len(),2);}
            let results=status_sequence.map(|s|vec![(s,0,b"\0\x01\0\0\0\0".as_slice())]).unwrap_or_default();
            peer.send(&[reply(call.call_id,&swap_answer(payload.fields,&results,!race.mixed()||unregistered))]).await;
        }
    }
}
struct RaceWire{root:String,port:u16,observed:oneshot::Receiver<()>,done:Option<oneshot::Sender<()>>,connections:Arc<AtomicUsize>,reads:Arc<AtomicUsize>,unhosts:Arc<AtomicUsize>,source:Option<tokio::task::JoinHandle<()>>,sdk:Option<tokio::task::JoinHandle<()>>,relay:Option<tokio::task::JoinHandle<()>>}
impl RaceWire{
    async fn open(race:Race)->Self{
        let cert=Certificates::generate("IP:127.0.0.1");let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=front.local_addr().unwrap().port();
        let boot=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let main=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let ports=[boot.local_addr().unwrap().port(),main.local_addr().unwrap().port()];
        let config=cert.server_config(&TLS13);let source_config=config.clone();let(done,finished)=oneshot::channel();let(observed,seen)=oneshot::channel();
        let connections=Arc::new(AtomicUsize::new(0));let counted=connections.clone();let reads=Arc::new(AtomicUsize::new(0));let unhosts=Arc::new(AtomicUsize::new(0));
        let relay=tokio::spawn(async move{
            let mut joins=Vec::new();for target in ports{let(mut incoming,_)=front.accept().await.unwrap();counted.fetch_add(1,Ordering::SeqCst);let mut outgoing=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,target)).await.unwrap();joins.push(tokio::spawn(async move{let _=tokio::io::copy_bidirectional(&mut incoming,&mut outgoing).await;}));}
            tokio::select!{result=front.accept()=>panic!("Client READ opened third connection: {}",result.is_ok()),result=finished=>result.unwrap()}
            assert!(tokio::time::timeout(Duration::from_millis(35),front.accept()).await.is_err());for join in &mut joins{tokio::time::timeout(JOIN_BOUND,join).await.unwrap().unwrap();}
        });
        let source=tokio::spawn(async move{let mut peer=CommonPeer::open(boot,source_config).await;let bytes=peer.next().await.unwrap();assert_eq!(bytes[0],0);let call=decode_request(&bytes[1..]).unwrap();assert_eq!(call.method,b"getAppConfigs.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"BootEntry");peer.send(&[reply(call.call_id,&bootstrap_client())]).await;peer.no_more_common().await;});
        let(captured,uri)=expected_login(&cert.root,port,Case::Reported);let sdk=tokio::spawn(Box::pin(strict_peer(main,config,race,captured,uri,observed,reads.clone(),unhosts.clone())));
        Self{root:cert.root.clone(),port,observed:seen,done:Some(done),connections,reads,unhosts,source:Some(source),sdk:Some(sdk),relay:Some(relay)}
    }
    async fn finish(&mut self,race:Race){
        self.done.take().unwrap().send(()).unwrap();
        // Await BORROWED handles in their owning slots. A cancelled wait keeps
        // the sole Join available; completed handles are removed, never repolled.
        for slot in [&mut self.source,&mut self.sdk,&mut self.relay]{if let Some(join)=slot.as_mut(){tokio::time::timeout(JOIN_BOUND,join).await.unwrap().unwrap();}*slot=None;}
        assert_eq!(self.connections.load(Ordering::SeqCst),2);assert_eq!(self.reads.load(Ordering::SeqCst),usize::from(!race.admission()&&!race.fatal()));
        assert_eq!(self.unhosts.load(Ordering::SeqCst),usize::from(!race.fatal()));assert!(self.source.is_none()&&self.sdk.is_none()&&self.relay.is_none());
    }
}
async fn run(race:Race){
    let mut wire=RaceWire::open(race).await;let mut profile=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&wire.root,wire.port),bootstrap_route()).unwrap();
    if race==Race::SharedBudget{let receivers=n::native_receiver::NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:Some(n::im_delivery::NativeImConfig{notification_timeout_ms:3000,receive_info:false,directory_queries:false})};profile=profile.with_native_receivers(Some(&receivers)).unwrap().with_client_ex(true).unwrap();}
    let denied=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(profile,denied.clone());let who=owner();
    backend.login(who.clone(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&wire.root)}).await.unwrap();let parent=backend.original_scope(&who).unwrap();backend.pause_original_polls_for_test(&who,&parent,true).unwrap();
    wait_callback(&mut events,&who,&parent,Callback::SdkLogin{success:true}).await;backend.update_status(who.clone(),STATUS_REGISTER,gateway()).await.unwrap();wait_callback(&mut events,&who,&parent,Callback::HostingAck{success:true}).await;
    let attempt=backend.original(&who).unwrap();assert!(attempt.bootstrap_handle.lock().await.is_none());assert!(!attempt.bootstrap_scope.as_ref().unwrap().is_open());let baseline=parent.native_rpc_resources();assert_eq!(baseline,(0,0));
    let mut retained_im=Vec::new();let receipt_gate=ImReceiptGate::fresh();
    if race==Race::SharedBudget{let commands=attempt.commands(&who,&parent).unwrap();for _ in 0..4{retained_im.push(commands.read_im(&who,&parent,Arc::new(n::rpc_im::ImQuery::refresh(&[],-1).unwrap()),receipt_gate.clone()).await.unwrap());}assert_eq!(parent.native_rpc_resources().0,4);}
    let action=UserEntryAction::original(who.clone(),parent.clone(),ImReceiptGate::fresh(),Instant::now()+if race==Race::PrefixTimeout{READ_DEADLINE}else{Duration::from_secs(3)},RECIPIENT.as_bytes()).unwrap();
    let mut external_reply=None;let mut already_unhosted=false;
    if race==Race::ReplyHolder{
        let target=attempt.user_entries.client_target().unwrap().unwrap();let reply=backend.resolve_client_user_entry(action.clone(),target).await.unwrap();observation(&reply.observe(&action).unwrap(),"phone","reported");external_reply=Some(Arc::new(reply));
        tokio::time::timeout(JOIN_BOUND,&mut wire.observed).await.unwrap().unwrap();
    }else{
        let control=ClientReadIoTest::original(if race.admission(){ClientReadTestPoint::Admission}else{ClientReadTestPoint::Prefix(1)});action.set_client_io_test(control.clone());let mut lookup=Box::pin(backend.resolve_user_entry(action.clone()));
        tokio::select!{result=&mut lookup=>panic!("lookup completed before actual actor/IO barrier: {result:?}"),_=async{if race.admission(){control.entered().await}else{control.paused().await}}=>{}}
        assert_eq!(control.sequence(),2);assert!(control.scope().matches(&parent));assert_eq!(attempt.user_entries.active(),0);assert_eq!(attempt.user_entries.retained_bytes(),0);
        assert_eq!(control.accepted.load(Ordering::SeqCst),usize::from(!race.admission()));assert_eq!(wire.reads.load(Ordering::SeqCst),0);
        let held=parent.native_rpc_resources();assert!(held.0>retained_im.len()&&held.1>0&&held.1<=128*1024);
        if race==Race::SharedBudget{
            let mut pressure=Vec::new();for _ in held.0..128{pressure.push(parent.reserve_native_rpc(1).unwrap());}assert_eq!(parent.native_rpc_resources().0,128);
            // Synchronous input admission must fail before copying or enqueue:
            // the main actor is still owning this immutable partial frame.
            assert_eq!(backend.reserve_user_entry_action(RECIPIENT.as_bytes(),ImReceiptGate::fresh(),Instant::now()+Duration::from_secs(1)).err(),Some("app_account_user_entry_capacity"));
            assert!(parent.is_open());assert_eq!(wire.reads.load(Ordering::SeqCst),0);drop(pressure);
            let byte_pressure=parent.reserve_native_rpc(128*1024-parent.native_rpc_resources().1).unwrap();assert!(parent.native_rpc_resources().0<128);assert_eq!(parent.native_rpc_resources().1,128*1024);
            assert_eq!(backend.reserve_user_entry_action(RECIPIENT.as_bytes(),ImReceiptGate::fresh(),Instant::now()+Duration::from_secs(1)).err(),Some("app_account_user_entry_capacity"));
            assert_eq!(control.accepted.load(Ordering::SeqCst),1);assert_eq!(wire.reads.load(Ordering::SeqCst),0);drop(byte_pressure);
        }
        match race{
            Race::PrefixTimeout=>{assert_eq!(tokio::time::timeout(JOIN_BOUND,lookup).await.unwrap().err(),Some("app_account_user_entry_timeout"));assert!(Instant::now()>=action.deadline());assert!(parent.is_open());assert_eq!(control.accepted.load(Ordering::SeqCst),1);control.release();},
            Race::PrefixStop=>{
                let mut stop=Box::pin(backend.update_status(who.clone(),STATUS_UNREGISTER,gateway()));
                tokio::select!{result=&mut lookup=>{assert_eq!(result.err(),Some("app_account_user_entry_scope_invalid"));},result=&mut stop=>panic!("unhosting overtook immutable partial Client frame: {result:?}")}
                drop(lookup);assert!(tokio::time::timeout(Duration::from_millis(10),stop.as_mut()).await.is_err());assert_eq!(wire.unhosts.load(Ordering::SeqCst),0);assert_eq!(control.accepted.load(Ordering::SeqCst),1);
                control.release();tokio::time::timeout(JOIN_BOUND,stop).await.unwrap().unwrap();already_unhosted=true;
            },
            Race::PrefixClose=>{parent.close();assert_eq!(lookup.await.err(),Some("app_account_user_entry_scope_invalid"));},
            Race::CleanupCancelled=>{
                drop(lookup);backend.retire(&who);
                {let mut cleanup=Box::pin(backend.logout(who.clone()));std::future::poll_fn(|cx|{assert!(cleanup.as_mut().poll(cx).is_pending());std::task::Poll::Ready(())}).await;}
                assert!(attempt.handle.try_lock().unwrap().is_some());assert!(!lock(&attempt.state).released);
                assert_eq!(backend.login(owner(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&wire.root)}).await,Err(BackendFailure::Unavailable));
            },
            _=>{drop(lookup);assert!(action.live()&&action.gate().live()&&parent.is_open());assert_eq!(parent.native_rpc_resources(),held);control.release();},
        }
        if !race.fatal(){tokio::time::timeout(JOIN_BOUND,&mut wire.observed).await.unwrap().unwrap();assert!(parent.is_open());if !already_unhosted{backend.update_status(who.clone(),STATUS_UNREGISTER,gateway()).await.unwrap();already_unhosted=true;}}
        if race.fatal(){tokio::time::timeout(JOIN_BOUND,async{while !control.dropped.load(Ordering::SeqCst){tokio::task::yield_now().await;}}).await.unwrap();assert_eq!(control.accepted.load(Ordering::SeqCst),1);assert!(!parent.is_open());}
        if !race.admission()&&!race.fatal(){assert!(control.accepted.load(Ordering::SeqCst)>1);}
    }
    if !race.fatal()&&!already_unhosted{backend.update_status(who.clone(),STATUS_UNREGISTER,gateway()).await.unwrap();}
    backend.logout(who.clone()).await.unwrap();if race==Race::CleanupCancelled{backend.logout(who).await.unwrap();}
    assert!(!parent.is_open()&&lock(&attempt.state).released);assert!(attempt.handle.lock().await.is_none()&&attempt.bootstrap_handle.lock().await.is_none()&&attempt.alive_handle.lock().await.is_none());
    assert_eq!(denied.calls.load(Ordering::SeqCst),0);wire.finish(race).await;drop(retained_im);drop(action);
    if let Some(reply)=external_reply{let last=reply.clone();let charged=parent.native_rpc_resources();assert!(charged.0>0&&charged.1>0);drop(reply);assert_eq!(parent.native_rpc_resources(),charged);drop(last);}
    assert_eq!(parent.native_rpc_resources(),baseline);
}
#[tokio::test]async fn client_actor_admission_cancel_retires_empty_outer_without_ack_and_joins(){Box::pin(run(Race::AdmissionEmpty)).await;}
#[tokio::test]async fn client_actor_admission_cancel_preserves_mixed_original_write_epoch_and_joins(){Box::pin(run(Race::AdmissionMixed)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_waiter_drop_finishes_owned_frame_and_joins(){Box::pin(run(Race::PrefixDrop)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_mixed_waiter_drop_keeps_write_epoch_and_joins(){Box::pin(run(Race::PrefixMixed)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_read_deadline_does_not_abort_original_frame(){Box::pin(run(Race::PrefixTimeout)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_stop_cannot_overtake_frame_then_unhosts_logout_joins(){Box::pin(run(Race::PrefixStop)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_scope_close_blocks_next_byte_before_drop_and_join(){Box::pin(run(Race::PrefixClose)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_cancelled_cleanup_retains_same_join_for_retry(){Box::pin(run(Race::CleanupCancelled)).await;}
#[tokio::test]async fn client_actor_actual_main_prefix_and_retained_im_share_original_budget_and_all_joins(){Box::pin(run(Race::SharedBudget)).await;}
#[tokio::test]async fn client_actor_reply_only_last_holder_keeps_charge_after_actual_io_release_and_all_joins(){Box::pin(run(Race::ReplyHolder)).await;}
