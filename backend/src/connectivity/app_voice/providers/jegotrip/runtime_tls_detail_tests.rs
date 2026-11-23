//! Actual original TCP/TLS/Common startup executed only by original Actions.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{self as n,startup_diagnostics::{StartupPhase,StartupStage},tls_failure_detail::TlsFailureReason};
async fn failing(kind:u8){
    let cert=Certificates::generate(if kind==0{"DNS:wrong.fixture.invalid"}else{"IP:127.0.0.1"});
    let other=Certificates::generate("IP:127.0.0.1");let root=if kind==1{&other.root}else{&cert.root};
    let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let config=cert.server_config(&TLS13);
    let worker=tokio::spawn(async move{
        let(tcp,_)=listener.accept().await.unwrap();
        if kind==2{
            let mut tcp=tcp;let mut hello=[0;4096];assert!(tcp.read(&mut hello).await.unwrap()>0);tcp.write_all(&[0xff,3,3,0,1,0]).await.unwrap();tcp.flush().await.unwrap();
        }else if let Ok(mut tls)=tokio_rustls::TlsAcceptor::from(config).accept(tcp).await{
            let mut payload=[0;32];assert!(matches!(tls.read(&mut payload).await,Ok(0)|Err(_)),"no Router/SDK payload after failed trust/name validation");
        }
    });
    let profile=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(root,port),bootstrap_route()).unwrap();let denied=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(profile,denied.clone());let who=owner();
    assert_eq!(Box::pin(backend.login(who.clone(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(root)})).await,Err(BackendFailure::Disconnected));
    let base=backend.startup_failure_observation(&who).unwrap();assert_eq!(base.phase,StartupPhase::Bootstrap);assert_eq!(base.stage,StartupStage::RouterTls);assert_eq!(base.code,"jegotrip_tls_handshake_failed");
    let detail=backend.startup_tls_detail_observation(&who).unwrap();assert_eq!(detail.phase,StartupPhase::Bootstrap);assert_eq!(detail.reason,match kind{0=>TlsFailureReason::NameMismatch,1=>TlsFailureReason::UnknownIssuer,_=>TlsFailureReason::MalformedRecord});
    assert!(backend.startup_tls_detail_observation(&owner()).is_none());assert_eq!(denied.calls.load(Ordering::SeqCst),0);assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));
    let json=serde_json::to_value(detail).unwrap();assert_eq!(json.as_object().unwrap().len(),2);for data in ["fixture","password","token","uid","hostname","certificate","root_der"]{assert!(!json.to_string().contains(data));}
    backend.logout(who.clone()).await.unwrap();assert!(lock(&backend.original(&who).unwrap().state).released);assert!(!backend.original_scope(&who).unwrap().is_open());worker.await.unwrap();
}
#[tokio::test]async fn runtime_tls_detail_actual_name_failure_retains_legacy_error_and_all_join(){Box::pin(failing(0)).await;}
#[tokio::test]async fn runtime_tls_detail_actual_wrong_issuer_retains_rejection_and_no_sdk_payload(){Box::pin(failing(1)).await;}
#[tokio::test]async fn runtime_tls_detail_actual_malformed_record_is_finite_without_router_or_sdk_payload(){Box::pin(failing(2)).await;}
#[tokio::test]
async fn runtime_tls_detail_successful_common_startup_never_fabricates_a_failure_category(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let policy=n::netstream_runtime::tests::policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let worker=tokio::spawn(async move{CommonPeer::open(listener,config).await.no_more_common().await;});let who=owner();let prepared=n::transport_actor::native::prepare_common(who.clone(),n::transport_actor::RuntimePolicy{io_timeout:Duration::from_secs(2),rpc_timeout:Duration::from_secs(2)},Instant::now()+Duration::from_secs(5)).unwrap();let trace=prepared.startup_trace();let scope=prepared.connection_scope();let reference=n::netstream_runtime::tests::reference(false);let n::object_reference::ReferenceTarget::Direct(endpoints)=reference.target()else{panic!("fixture endpoint")};
    let connected=Box::pin(prepared.connect(&policy,&[],reference.identity(),&endpoints[0],0,n::netstream_runtime::tests::timeouts(),None)).await.unwrap();assert!(trace.snapshot(&who,&scope,StartupPhase::Account).is_none());assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());
    let(mut handle,mut events)=connected.attach().unwrap();assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));assert!(handle.close().await.io_released());assert!(!scope.is_open());worker.await.unwrap();
}
#[tokio::test]
async fn runtime_tls_detail_bootstrap_account_projection_keeps_first_phase_and_clears_only_after_stop_join(){
    let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=tokio::spawn(no_payload_on_bad_name(listener,cert.server_config(&TLS13)));
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let created=manager.create(bootstrap_account_config(&cert.root,port),100).await.unwrap();let line=created.account.line_id;
    assert!(created.startup_tls_detail.is_none());assert!(manager.start(&line,1,100).await.is_err());let failed=manager.get(&line,100).await.unwrap();assert_eq!(failed.phase,"disconnected");assert!(failed.io_cleanup_required);
    let base=failed.startup_failure.unwrap();let detail=failed.startup_tls_detail.unwrap();assert_eq!(base.phase,StartupPhase::Bootstrap);assert_eq!(base.stage,StartupStage::RouterTls);assert_eq!(base.code,"jegotrip_tls_handshake_failed");assert_eq!(detail.phase,base.phase);assert_eq!(detail.reason,TlsFailureReason::NameMismatch);
    assert!(!failed.incoming_message_ready&&!failed.incoming_call_ready&&!failed.outbound_authorized);assert_eq!(failed.provider_availability,"unsupported");assert!(failed.account_alive_state.is_none());
    let encoded=serde_json::to_string(&failed).unwrap();for private in ["wrong.fixture","root_der","certificate","phone_login_token","private_config"]{assert!(!encoded.contains(private));}
    assert_eq!(manager.list(100).await.unwrap()[0].startup_tls_detail,Some(detail));manager.stop(&line,1,100).await.unwrap();let stopped=manager.get(&line,100).await.unwrap();assert_eq!(stopped.phase,"stopped");assert!(!stopped.io_cleanup_required);assert!(stopped.startup_failure.is_none()&&stopped.startup_tls_detail.is_none());
    assert_eq!(temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases",[],|r|r.get::<_,i64>(0)).map_err(|_|"test_query_failed")).unwrap(),0);server.await.unwrap();
}
