//! Actions-only scoped diagnostic data and real startup failure/cancellation.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{startup_diagnostics::{StartupTrace,StartupFailure,StartupPhase,StartupStage},
    runtime_connector::ConnectError,router_runtime::RouterIoError,native_router::RouterConnectError,
    netstream_runtime::NetstreamError,common_runtime::CommonIoError,tls_engine::TlsError};
use std::sync::Arc;

#[test]
fn startup_trace_without_failure_neither_grants_nor_retires_the_original_scope() {
    let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
    assert!(trace.snapshot(&original,&scope,StartupPhase::Account).is_none());assert!(scope.is_open());
    drop(trace);assert!(scope.is_open());scope.close();
}
#[test]
fn specific_startup_failure_survives_scope_close_without_being_replaced_by_generic_closed() {
    let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
    scope.close();trace.router(&original,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    trace.runtime(&original,&scope,RuntimeError::Closed);
    assert_eq!(trace.snapshot(&original,&scope,StartupPhase::Bootstrap),Some(StartupFailure {phase:StartupPhase::Bootstrap,
        stage:StartupStage::RouterTls,code:"jegotrip_tls_handshake_failed"}));
    assert!(!scope.is_open());assert!(!scope.session_revoked());
}
#[test]
fn startup_trace_rejects_an_equal_looking_new_owner_or_foreign_scope_on_write_and_read() {
    let original=owner();let foreign=owner();let scope=RpcConnectionScope::fresh();let other=RpcConnectionScope::fresh();
    let trace=StartupTrace::original(original.clone(),scope.clone());
    trace.runtime(&foreign,&scope,RuntimeError::Io);trace.runtime(&original,&other,RuntimeError::Timeout);
    assert!(trace.snapshot(&original,&scope,StartupPhase::Account).is_none());
    trace.router(&original,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert!(trace.snapshot(&foreign,&scope,StartupPhase::Account).is_none());
    assert!(trace.snapshot(&original,&other,StartupPhase::Account).is_none());assert!(scope.is_open());
}
#[test]
fn startup_router_failure_categories_do_not_conflate_tcp_tls_and_router_rejection() {
    for (error,stage,code) in [(RouterIoError::Connect(ConnectError::Io),StartupStage::RouterConnect,"jegotrip_connector_io_failed"),
        (RouterIoError::Tls(TlsError::HandshakeFailed),StartupStage::RouterTls,"jegotrip_tls_handshake_failed"),
        (RouterIoError::Control(RouterConnectError::Rejected),StartupStage::RouterControl,"jegotrip_router_control_rejected"),
        (RouterIoError::Io,StartupStage::RouterIo,"jegotrip_router_io_failed")] {
        let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
        trace.router(&original,&scope,error);let result=trace.snapshot(&original,&scope,StartupPhase::Account).unwrap();
        assert_eq!(result.stage,stage);assert_eq!(result.code,code);assert!(scope.is_open());
    }
}
#[test]
fn startup_logical_channel_and_common_errors_are_distinct_data_only() {
    let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
    trace.netstream(&original,&scope,NetstreamError::Timeout);
    assert_eq!(trace.snapshot(&original,&scope,StartupPhase::Account).unwrap().stage,StartupStage::LogicalChannel);
    let trace=StartupTrace::original(original.clone(),scope.clone());trace.common(&original,&scope,CommonIoError::Configuration);
    let result=trace.snapshot(&original,&scope,StartupPhase::Account).unwrap();assert_eq!(result.stage,StartupStage::CommonFraming);
    assert_eq!(result.code,"jegotrip_common_startup_configuration_invalid");assert!(scope.is_open());
}
#[test]
fn startup_failure_serialization_contains_only_finite_phase_stage_and_code() {
    let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
    trace.runtime(&original,&scope,RuntimeError::InvalidConfiguration);
    let value=trace.snapshot(&original,&scope,StartupPhase::Bootstrap).unwrap();let text=serde_json::to_string(&value).unwrap();
    let json:serde_json::Value=serde_json::from_str(&text).unwrap();assert_eq!(json.as_object().unwrap().len(),3);
    assert_eq!(json["phase"],"bootstrap");assert_eq!(json["stage"],"preparation");
    for private in ["fixture","token","password","hostname","certificate","root_der","owner","scope","ready"] {assert!(!text.contains(private));}
    assert_eq!(format!("{trace:?}"),"StartupTrace(<redacted>)");assert!(text.len()<192);
}
#[test]
fn first_startup_failure_is_single_slot_and_cannot_be_overwritten_by_concurrent_followups() {
    let original=owner();let scope=RpcConnectionScope::fresh();let trace=Arc::new(StartupTrace::original(original.clone(),scope.clone()));
    trace.router(&original,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    let threads:Vec<_>=(0..2).map(|_|{let trace=trace.clone();let original=original.clone();let scope=scope.clone();
        std::thread::spawn(move||for _ in 0..100 {trace.runtime(&original,&scope,RuntimeError::Closed);})}).collect();
    for thread in threads {thread.join().unwrap();}
    assert_eq!(trace.snapshot(&original,&scope,StartupPhase::Account).unwrap().code,"jegotrip_tls_handshake_failed");
}
#[test]
fn runtime_diagnostic_codes_are_bounded_static_labels_not_formatted_errors() {
    for error in [RuntimeError::Scope,RuntimeError::InvalidConfiguration,RuntimeError::Backpressure,RuntimeError::Closed,
        RuntimeError::Cancelled,RuntimeError::Timeout,RuntimeError::Io,RuntimeError::Protocol,RuntimeError::SessionRevoked,
        RuntimeError::CallNotReady,RuntimeError::CallRejected,RuntimeError::UnregistrationRejected,RuntimeError::LoggedOut,
        RuntimeError::Tls(TlsError::HandshakeFailed)] {
        let original=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(original.clone(),scope.clone());
        trace.runtime(&original,&scope,error);let value=trace.snapshot(&original,&scope,StartupPhase::Account).unwrap();
        assert!(value.code.len()<=64 && value.code.bytes().all(|b|b.is_ascii_lowercase()||b==b'_'));
        assert_eq!(value.stage,match error {RuntimeError::Scope|RuntimeError::InvalidConfiguration=>StartupStage::Preparation,_=>StartupStage::Attempt});
        assert!(scope.is_open());
    }
}

#[tokio::test]
async fn rejected_native_preparation_records_configuration_without_contacting_router() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let original=owner();
    let prepared=prepare_common(original.clone(),rpc_policy(),Instant::now()+Duration::from_secs(4)).unwrap();
    let trace=prepared.startup_trace();let scope=prepared.connection_scope();let reference=reference(false);
    let ReferenceTarget::Direct(endpoints)=reference.target() else{panic!("endpoint")};
    let error=prepared.connect(&policy,&[],reference.identity(),&endpoints[0],1,timeouts(),None).await.err().unwrap();
    assert_eq!(error,RuntimeError::InvalidConfiguration);assert!(!scope.is_open());
    assert_eq!(trace.snapshot(&original,&scope,StartupPhase::Account).unwrap().stage,StartupStage::Preparation);
    assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
}
#[tokio::test]
async fn native_name_mismatch_retains_router_tls_diagnostic_without_sending_router_or_sdk_payload() {
    let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);
    let worker=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move {
        let(tcp,_)=listener.accept().await.unwrap();let accepted=tokio_rustls::TlsAcceptor::from(server).accept(tcp).await;
        if let Ok(mut tls)=accepted {let mut data=[0;16];assert!(matches!(tls.read(&mut data).await,Ok(0)|Err(_)));}
    }).await.unwrap()});
    let original=owner();let prepared=prepare_common(original.clone(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();
    let trace=prepared.startup_trace();let scope=prepared.connection_scope();let reference=reference(false);
    let ReferenceTarget::Direct(endpoints)=reference.target() else{panic!("endpoint")};
    let error=prepared.connect(&policy,&[],reference.identity(),&endpoints[0],0,timeouts(),None).await.err().unwrap();
    assert!(matches!(error,RuntimeError::Tls(TlsError::HandshakeFailed)|RuntimeError::Closed));assert!(!scope.is_open());
    assert_eq!(trace.snapshot(&original,&scope,StartupPhase::Account),Some(StartupFailure {phase:StartupPhase::Account,
        stage:StartupStage::RouterTls,code:"jegotrip_tls_handshake_failed"}));worker.await.unwrap();
}
#[tokio::test]
async fn successful_common_startup_has_no_failure_and_does_not_authenticate_the_sdk() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move {
        CommonPeer::open(listener,config).await.no_more_common().await;
    }).await.unwrap()});
    let original=owner();let prepared=prepare_common(original.clone(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();
    let trace=prepared.startup_trace();let scope=prepared.connection_scope();let reference=reference(false);
    let ReferenceTarget::Direct(endpoints)=reference.target() else{panic!("endpoint")};
    let connected=prepared.connect(&policy,&[],reference.identity(),&endpoints[0],0,timeouts(),None).await.unwrap();
    assert!(trace.snapshot(&original,&scope,StartupPhase::Account).is_none());
    let(mut handle,mut events)=connected.attach().unwrap();assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));
    assert!(handle.close().await.io_released());assert!(!scope.is_open());
    assert!(trace.snapshot(&original,&scope,StartupPhase::Account).is_none());server.await.unwrap();
}

#[tokio::test]
async fn cancelled_native_startup_closes_io_but_does_not_fabricate_an_observed_failure_or_grant() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let original=owner();
    let prepared=prepare_common(original.clone(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();
    let trace=prepared.startup_trace();let scope=prepared.connection_scope();
    let task=tokio::spawn(async move {let reference=reference(false);let ReferenceTarget::Direct(endpoints)=reference.target() else{panic!("endpoint")};
        prepared.connect(&policy,&[],reference.identity(),&endpoints[0],0,timeouts(),None).await});
    let(mut tcp,_)=listener.accept().await.unwrap();let mut bytes=[0;4096];let count=tcp.read(&mut bytes).await.unwrap();
    assert!(count>0);assert_eq!(bytes[0],22);task.abort();assert!(task.await.unwrap_err().is_cancelled());
    assert!(!scope.is_open());assert!(trace.snapshot(&original,&scope,StartupPhase::Account).is_none());
    tokio::time::timeout(Duration::from_secs(2),async{loop{match tcp.read(&mut bytes).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}}).await.unwrap();
}
