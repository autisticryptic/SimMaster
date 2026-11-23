//! Original Actions additional original-scope timeout data; old3-field/TLS data unchanged.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{startup_diagnostics::{StartupTrace,StartupPhase,StartupStage},connect_failure_detail::ConnectFailureStep,rpc_pending::RpcConnectionScope,runtime_connector::ConnectError,router_runtime::RouterIoError,tls_engine::TlsError};
#[test]
fn startup_connect_detail_generic_timeout_without_connector_evidence_does_not_guess_step(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert!(trace.connect_detail(&who,&scope,StartupPhase::Bootstrap).is_none());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap().code,"jegotrip_connector_timeout");assert!(scope.is_open());
}
#[test]
fn startup_connect_detail_first_step_matches_only_original_owner_and_scope(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&who,ConnectFailureStep::Dns);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert_eq!(trace.connect_detail(&who,&scope,StartupPhase::Bootstrap).unwrap().step,ConnectFailureStep::Dns);assert!(trace.connect_detail(&owner(),&scope,StartupPhase::Bootstrap).is_none());assert!(trace.connect_detail(&who,&RpcConnectionScope::fresh(),StartupPhase::Bootstrap).is_none());
}
#[test]
fn startup_connect_detail_foreign_owner_record_cannot_label_original_timeout(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&owner(),ConnectFailureStep::Tcp);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert!(trace.connect_detail(&who,&scope,StartupPhase::Account).is_none());
}
#[test]
fn startup_connect_detail_original_closed_scope_retains_data_but_not_readiness(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&who,ConnectFailureStep::Tcp);scope.close();trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));let data=trace.connect_detail(&who,&scope,StartupPhase::Account).unwrap();assert_eq!(data.phase,StartupPhase::Account);assert_eq!(data.step,ConnectFailureStep::Tcp);assert!(!scope.is_open());
}
#[test]
fn startup_connect_detail_late_step_cannot_backfill_first_generic_timeout(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));scope.record_connect_timeout(&who,ConnectFailureStep::Dns);assert!(trace.connect_detail(&who,&scope,StartupPhase::Account).is_none());
}
#[test]
fn startup_connect_detail_prior_tls_failure_never_acquires_connector_step(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_tls_detail(&who,crate::connectivity::app_voice::providers::jegotrip::tls_failure_detail::TlsFailureReason::NameMismatch);trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));scope.record_connect_timeout(&who,ConnectFailureStep::Tcp);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert!(trace.connect_detail(&who,&scope,StartupPhase::Account).is_none());assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_some());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Account).unwrap().stage,StartupStage::RouterTls);
}
#[test]
fn startup_connect_detail_connect_io_failure_is_not_a_deadline_observation(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&who,ConnectFailureStep::Dns);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Io));assert!(trace.connect_detail(&who,&scope,StartupPhase::Bootstrap).is_none());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap().code,"jegotrip_connector_io_failed");
}
#[test]
fn startup_connect_detail_separate_two_field_projection_never_alters_legacy_three_fields(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&who,ConnectFailureStep::Dns);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));let base=serde_json::to_value(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap()).unwrap();assert_eq!(base.as_object().unwrap().len(),3);let data=serde_json::to_value(trace.connect_detail(&who,&scope,StartupPhase::Bootstrap).unwrap()).unwrap();assert_eq!(data.as_object().unwrap().len(),2);assert_eq!(data["step"],"dns");for private in ["owner","scope","token","certificate","hostname","address","timeout_ms","ready"]{assert!(!data.to_string().contains(private));}
}
#[test]
fn startup_connect_detail_whole_runtime_timeout_does_not_promote_later_connector_data(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());trace.runtime(&who,&scope,RuntimeError::Timeout);scope.record_connect_timeout(&who,ConnectFailureStep::Dns);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert!(trace.connect_detail(&who,&scope,StartupPhase::Account).is_none());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Account).unwrap().stage,StartupStage::Attempt);
}
#[test]
fn startup_connect_detail_first_dns_step_is_not_replaced_by_later_tcp_followup(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_connect_timeout(&who,ConnectFailureStep::Dns);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));scope.record_connect_timeout(&who,ConnectFailureStep::Tcp);trace.router(&who,&scope,RouterIoError::Connect(ConnectError::Timeout));assert_eq!(trace.connect_detail(&who,&scope,StartupPhase::Account).unwrap().step,ConnectFailureStep::Dns);assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());
}
