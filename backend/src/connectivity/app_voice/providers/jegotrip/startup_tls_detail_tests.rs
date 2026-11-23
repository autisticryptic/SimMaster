//! Original Actions SOURCE only; additional detail never changes legacy3-field data.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{startup_diagnostics::{StartupTrace,StartupPhase,StartupStage},tls_failure_detail::TlsFailureReason,rpc_pending::RpcConnectionScope,router_runtime::RouterIoError,tls_engine::TlsError};
#[test]
fn startup_tls_detail_without_engine_observation_cannot_infer_name_or_ca_from_generic_code(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());
    trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));assert!(trace.tls_detail(&who,&scope,StartupPhase::Bootstrap).is_none());
    assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap().code,"jegotrip_tls_handshake_failed");assert!(scope.is_open());
}
#[test]
fn startup_tls_detail_is_original_scope_and_owner_data_only(){
    let who=owner();let foreign=owner();let scope=RpcConnectionScope::fresh();let other=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());
    scope.record_tls_detail(&who,TlsFailureReason::NameMismatch);trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert_eq!(trace.tls_detail(&who,&scope,StartupPhase::Bootstrap).unwrap().reason,TlsFailureReason::NameMismatch);
    assert!(trace.tls_detail(&foreign,&scope,StartupPhase::Bootstrap).is_none());assert!(trace.tls_detail(&who,&other,StartupPhase::Bootstrap).is_none());assert!(scope.is_open()&&!scope.session_revoked());
}
#[test]
fn startup_tls_detail_never_promotes_another_scopes_or_owners_record(){
    let who=owner();let foreign=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());
    scope.record_tls_detail(&foreign,TlsFailureReason::UnknownIssuer);trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());
}
#[test]
fn startup_tls_detail_first_non_tls_failure_is_not_replaced_by_later_tls_record(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());trace.runtime(&who,&scope,RuntimeError::Timeout);
    scope.record_tls_detail(&who,TlsFailureReason::NameMismatch);trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Account).unwrap().stage,StartupStage::Attempt);
}
#[test]
fn startup_tls_detail_read_does_not_use_new_late_scope_reason_as_first_failure_evidence(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    scope.record_tls_detail(&who,TlsFailureReason::NameMismatch);assert!(trace.tls_detail(&who,&scope,StartupPhase::Account).is_none());
}
#[test]
fn startup_tls_detail_survives_original_close_without_ready_or_new_scope(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_tls_detail(&who,TlsFailureReason::UnknownIssuer);
    scope.close();trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert_eq!(trace.tls_detail(&who,&scope,StartupPhase::Account).unwrap().reason,TlsFailureReason::UnknownIssuer);assert!(!scope.is_open());assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn startup_tls_detail_separate_projection_does_not_change_legacy_three_fields(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_tls_detail(&who,TlsFailureReason::NameMismatch);trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    let base=serde_json::to_value(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap()).unwrap();assert_eq!(base.as_object().unwrap().len(),3);
    let detail=serde_json::to_value(trace.tls_detail(&who,&scope,StartupPhase::Bootstrap).unwrap()).unwrap();assert_eq!(detail.as_object().unwrap().len(),2);assert_eq!(detail["reason"],"name_mismatch");
    for value in ["owner","scope","token","password","certificate","hostname","root_der","ready"]{assert!(!detail.to_string().contains(value));}
}
#[test]
fn startup_tls_detail_is_single_first_reason_and_missing_for_non_handshake_tls_error(){
    let who=owner();let scope=RpcConnectionScope::fresh();let trace=StartupTrace::original(who.clone(),scope.clone());scope.record_tls_detail(&who,TlsFailureReason::NameMismatch);
    trace.router(&who,&scope,RouterIoError::Tls(TlsError::Timeout));trace.router(&who,&scope,RouterIoError::Tls(TlsError::HandshakeFailed));
    assert!(trace.tls_detail(&who,&scope,StartupPhase::Bootstrap).is_none());assert_eq!(trace.snapshot(&who,&scope,StartupPhase::Bootstrap).unwrap().code,"jegotrip_tls_deadline_expired");
}
