//! Actions-only native bootstrap/account projection of actual connector timeout.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{self as n,connect_failure_detail::ConnectFailureStep as Step,
    connect_phase_test_support::ConnectDetailFactory,session::HostingBackend};
use tokio::io::AsyncReadExt;

async fn bootstrap_timeout(step:Step){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut profile=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,listener.local_addr().unwrap().port()),bootstrap_route()).unwrap();
    let gate=profile.connect_detail_test_gate(step).unwrap();let denied=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(profile,denied.clone());let who=owner();
    assert_eq!(Box::pin(backend.login(who.clone(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&cert.root)})).await,Err(BackendFailure::Disconnected));
    let base=backend.startup_failure_observation(&who).unwrap();let detail=backend.startup_connect_detail_observation(&who).unwrap();assert_eq!(base.phase,StartupPhase::Bootstrap);assert_eq!(base.stage,StartupStage::RouterConnect);assert_eq!(base.code,"jegotrip_connector_timeout");assert_eq!(detail.phase,base.phase);assert_eq!(detail.step,step);
    assert!(backend.startup_tls_detail_observation(&who).is_none());assert!(backend.startup_connect_detail_observation(&owner()).is_none());assert_eq!(denied.calls.load(Ordering::Acquire),0);assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));
    let(original,scope)=gate.original_scope().unwrap();assert_eq!(original,who);assert!(!scope.is_open());assert_eq!(scope.connect_timeout_for(&who),Some(step));assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
    backend.logout(who.clone()).await.unwrap();assert!(lock(&backend.original(&who).unwrap().state).released);gate.release();
}
#[tokio::test]
async fn runtime_connect_detail_bootstrap_dns_hold_is_finite_and_no_sdk_or_router_payload(){Box::pin(bootstrap_timeout(Step::Dns)).await;}
#[tokio::test]
async fn runtime_connect_detail_bootstrap_tcp_hold_is_not_a_tls_failure_and_joins(){Box::pin(bootstrap_timeout(Step::Tcp)).await;}
#[tokio::test]
async fn runtime_connect_detail_successful_common_startup_has_no_connector_or_tls_category(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let policy=n::netstream_runtime::tests::policy(&cert.root,listener.local_addr().unwrap().port());let cfg=cert.server_config(&TLS13);
    let server=tokio::spawn(async move{CommonPeer::open(listener,cfg).await.no_more_common().await;});let who=owner();let prepared=n::transport_actor::native::prepare_common(who.clone(),n::transport_actor::RuntimePolicy{io_timeout:Duration::from_secs(2),rpc_timeout:Duration::from_secs(2)},Instant::now()+Duration::from_secs(5)).unwrap();let trace=prepared.startup_trace();let scope=prepared.connection_scope();let reference=n::netstream_runtime::tests::reference(false);let n::object_reference::ReferenceTarget::Direct(endpoints)=reference.target()else{panic!("endpoint fixture")};
    let connected=Box::pin(prepared.connect(&policy,&[],reference.identity(),&endpoints[0],0,n::netstream_runtime::tests::timeouts(),None)).await.unwrap();assert!(trace.snapshot(&who,&scope,StartupPhase::Account).is_none());assert!(trace.connect_detail(&who,&scope,StartupPhase::Account).is_none());assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());
    let(mut handle,mut events)=connected.attach().unwrap();assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));assert!(handle.close().await.io_released());assert!(!scope.is_open());server.await.unwrap();
}
async fn account_timeout(step:Step){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let temp=TempDatabase::new();let factory=ConnectDetailFactory::for_step(step);let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let cfg=bootstrap_account_config(&cert.root,listener.local_addr().unwrap().port());let timeout=cfg.profile.connect_timeout_ms;
    let created=manager.create(cfg,100).await.unwrap();let line=created.account.line_id;assert!(created.startup_connect_detail.is_none());let start=Instant::now();assert!(manager.start(&line,1,100).await.is_err());assert!(start.elapsed()>=Duration::from_millis(timeout));assert_eq!(factory.builds(),1);
    let view=manager.get(&line,100).await.unwrap();assert_eq!(view.phase,"disconnected");assert!(view.io_cleanup_required);let detail=view.startup_connect_detail.unwrap();assert_eq!(detail.phase,StartupPhase::Bootstrap);assert_eq!(detail.step,step);assert_eq!(view.startup_failure.unwrap().code,"jegotrip_connector_timeout");assert!(view.startup_tls_detail.is_none());assert!(!view.incoming_message_ready&&!view.incoming_call_ready&&!view.outbound_authorized);
    let json=serde_json::to_value(&view).unwrap();assert_eq!(json["startup_failure"].as_object().unwrap().len(),3);assert_eq!(json["startup_connect_detail"].as_object().unwrap().len(),2);for secret in ["fixture","root_der","hostname","certificate","phone_login_token","private_config"]{assert!(!json.to_string().contains(secret));}
    assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());manager.stop(&line,1,100).await.unwrap();let stopped=manager.get(&line,100).await.unwrap();assert_eq!(stopped.phase,"stopped");assert!(!stopped.io_cleanup_required);assert!(stopped.startup_connect_detail.is_none()&&stopped.startup_failure.is_none());
    assert_eq!(temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases",[],|r|r.get::<_,i64>(0)).map_err(|_|"test_query_failed")).unwrap(),0);factory.gate().release();
}
#[tokio::test]
async fn account_connect_detail_dns_timeout_keeps_original_policy_and_stop_clears_data(){Box::pin(account_timeout(Step::Dns)).await;}
#[tokio::test]
async fn account_connect_detail_tcp_timeout_remains_disconnected_until_stop_and_all_joins(){Box::pin(account_timeout(Step::Tcp)).await;}
#[tokio::test]
async fn runtime_connect_detail_cancelled_bootstrap_never_promotes_unreported_deadline(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut profile=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,listener.local_addr().unwrap().port()),bootstrap_route()).unwrap();let gate=profile.connect_detail_test_gate(Step::Dns).unwrap();let(backend,_events)=NativeHostingBackend::new(profile,Arc::new(LegacyDenied{calls:AtomicUsize::new(0)}));let who=owner();let copy=backend.clone();let original=who.clone();let root=cert.root.clone();
    let task=tokio::spawn(async move{Box::pin(copy.login(original,SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&root)})).await});gate.entered().await;task.abort();assert!(task.await.unwrap_err().is_cancelled());assert!(backend.startup_connect_detail_observation(&who).is_none());assert!(backend.startup_failure_observation(&who).is_none());let(_,scope)=gate.original_scope().unwrap();assert!(!scope.is_open());
    backend.logout(who.clone()).await.unwrap();assert!(lock(&backend.original(&who).unwrap().state).released);assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());gate.release();
}
