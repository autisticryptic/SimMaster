//! Actions-only runtime/account projection of original startup failures.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{startup_diagnostics::{StartupPhase,StartupStage},
    account_runtime::AccountManager,account_test_support::TempDatabase};

async fn no_payload_on_bad_name(listener:TcpListener,server:Arc<tokio_rustls::rustls::ServerConfig>) {
    tokio::time::timeout(Duration::from_secs(8),async move {
        let (tcp,_)=listener.accept().await.unwrap();
        if let Ok(mut tls)=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await {
            let mut bytes=[0;32];assert!(matches!(tls.read(&mut bytes).await,Ok(0)|Err(_)));
        }
    }).await.unwrap();
}

#[tokio::test]
async fn bootstrap_startup_diagnostic_survives_retirement_only_for_its_original_owner() {
    let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server=tokio::spawn(no_payload_on_bad_name(listener,cert.server_config(&TLS13)));
    let profile=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,port),bootstrap_route()).unwrap();
    let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(profile,connector.clone());
    let original=owner();
    assert_eq!(backend.login(original.clone(),SdkLoginRequest {server_address:BOOTSTRAP_SDK,..request(&cert.root)}).await,Err(BackendFailure::Disconnected));
    let diagnostic=backend.startup_failure_observation(&original).unwrap();
    assert_eq!(diagnostic.phase,StartupPhase::Bootstrap);assert_eq!(diagnostic.stage,StartupStage::RouterTls);
    assert_eq!(diagnostic.code,"jegotrip_tls_handshake_failed");
    assert!(backend.startup_failure_observation(&owner()).is_none());assert!(!backend.original_scope(&original).unwrap().is_open());
    assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));assert_eq!(connector.calls.load(Ordering::Acquire),0);
    assert!(backend.account_alive_observation(&original).is_none());backend.logout(original.clone()).await.unwrap();
    assert!(lock(&backend.original(&original).unwrap().state).released);server.await.unwrap();
}

#[tokio::test]
async fn direct_common_account_startup_failure_is_not_mislabeled_as_bootstrap() {
    let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server=tokio::spawn(no_payload_on_bad_name(listener,cert.server_config(&TLS13)));
    let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,_events)=NativeHostingBackend::new(profile(&cert.root,port),connector.clone());
    let original=owner();assert_eq!(backend.login(original.clone(),request(&cert.root)).await,Err(BackendFailure::Disconnected));
    let diagnostic=backend.startup_failure_observation(&original).unwrap();
    assert_eq!(diagnostic.phase,StartupPhase::Account);assert_eq!(diagnostic.stage,StartupStage::RouterTls);
    assert_eq!(diagnostic.code,"jegotrip_tls_handshake_failed");assert_eq!(connector.calls.load(Ordering::Acquire),0);
    backend.logout(original).await.unwrap();server.await.unwrap();
}

#[tokio::test]
async fn account_view_exposes_bounded_startup_failure_without_ready_and_stop_does_not_restore_it() {
    let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server=tokio::spawn(no_payload_on_bad_name(listener,cert.server_config(&TLS13)));
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());
    let created=manager.create(bootstrap_account_config(&cert.root,port),100).await.unwrap();let line=created.account.line_id;
    assert!(created.startup_failure.is_none());assert!(manager.start(&line,1,100).await.is_err());
    let failed=manager.get(&line,100).await.unwrap();assert_eq!(failed.phase,"disconnected");assert!(failed.io_cleanup_required);
    let diagnostic=failed.startup_failure.unwrap();assert_eq!(diagnostic.phase,StartupPhase::Bootstrap);
    assert_eq!(diagnostic.stage,StartupStage::RouterTls);assert_eq!(diagnostic.code,"jegotrip_tls_handshake_failed");
    assert!(!failed.incoming_message_ready);assert!(!failed.incoming_call_ready);assert!(!failed.outbound_authorized);
    assert_eq!(failed.provider_availability,"unsupported");assert!(failed.account_alive_state.is_none());
    let encoded=serde_json::to_string(&failed).unwrap();
    for value in ["fixture","wrong.","phone_login_token","root_der","certificate","private_config","token"] {assert!(!encoded.contains(value));}
    manager.stop(&line,1,100).await.unwrap();let stopped=manager.get(&line,100).await.unwrap();
    assert_eq!(stopped.phase,"stopped");assert!(!stopped.io_cleanup_required);assert!(stopped.startup_failure.is_none());
    assert!(!stopped.incoming_message_ready);assert!(!stopped.incoming_call_ready);
    assert_eq!(temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases",[],|r|r.get::<_,i64>(0)).map_err(|_|"test_query_failed")).unwrap(),0);
    assert_eq!(manager.list(100).await.unwrap().len(),1);server.await.unwrap();
}
