//! Original Actions SOURCE; finite DNS/TCP observations do not change IO or budgets.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use std::sync::Arc;
fn owner()->AttemptOwner{let mut s=n::session::HostingSession::new("fixture-connect-detail".into());s.import(n::tests::imported(),100).unwrap();s.begin_login(100).unwrap()}
#[test]
fn connect_detail_steps_are_only_finite_dns_tcp_not_endpoint_or_error_text(){
    assert_eq!(serde_json::to_string(&ConnectFailureStep::Dns).unwrap(),"\"dns\"");assert_eq!(serde_json::to_string(&ConnectFailureStep::Tcp).unwrap(),"\"tcp\"");
    assert_eq!(ConnectFailureStep::from_byte(0),None);assert_eq!(ConnectFailureStep::from_byte(255),None);
}
#[test]
fn connect_detail_timeout_before_any_observed_step_never_guesses_dns_or_tcp(){
    let who=owner();let scope=RpcConnectionScope::fresh();let attempt=ConnectAttempt::original(&who,&scope);attempt.finish(ConnectError::Timeout);assert_eq!(scope.connect_timeout_for(&who),None);assert!(scope.is_open());
}
#[test]
fn connect_detail_io_scope_resource_and_configuration_errors_do_not_become_timeout_steps(){
    for error in [ConnectError::Io,ConnectError::Scope,ConnectError::ResourceLimit,ConnectError::InvalidConfiguration,ConnectError::UnsupportedProfile]{let who=owner();let scope=RpcConnectionScope::fresh();let a=ConnectAttempt::original(&who,&scope);a.entering(ConnectFailureStep::Dns);a.finish(error);assert_eq!(scope.connect_timeout_for(&who),None);assert!(scope.is_open());}
}
#[test]
fn connect_detail_records_dns_only_when_original_connector_reports_timeout(){
    let who=owner();let scope=RpcConnectionScope::fresh();let a=ConnectAttempt::original(&who,&scope);a.entering(ConnectFailureStep::Dns);assert_eq!(scope.connect_timeout_for(&who),None);a.finish(ConnectError::Timeout);assert_eq!(scope.connect_timeout_for(&who),Some(ConnectFailureStep::Dns));assert!(scope.is_open());
}
#[test]
fn connect_detail_last_real_connector_step_is_tcp_without_storing_dns_result(){
    let who=owner();let scope=RpcConnectionScope::fresh();let a=ConnectAttempt::original(&who,&scope);a.entering(ConnectFailureStep::Dns);a.entering(ConnectFailureStep::Tcp);a.finish(ConnectError::Timeout);assert_eq!(scope.connect_timeout_for(&who),Some(ConnectFailureStep::Tcp));
}
#[test]
fn connect_detail_store_first_owner_and_first_timeout_step_are_not_rewritten(){
    let who=owner();let foreign=owner();let store=ConnectDiagnosticStore::original();store.record(&who,ConnectFailureStep::Dns);store.record(&who,ConnectFailureStep::Tcp);store.record(&foreign,ConnectFailureStep::Tcp);assert_eq!(store.for_owner(&who),Some(ConnectFailureStep::Dns));assert_eq!(store.for_owner(&foreign),None);
}
#[test]
fn connect_detail_scope_clones_share_history_and_other_scope_does_not(){
    let who=owner();let scope=RpcConnectionScope::fresh();let copy=scope.clone();let other=RpcConnectionScope::fresh();scope.record_connect_timeout(&who,ConnectFailureStep::Tcp);assert_eq!(copy.connect_timeout_for(&who),Some(ConnectFailureStep::Tcp));assert_eq!(other.connect_timeout_for(&who),None);assert_eq!(scope.connect_timeout_for(&owner()),None);
}
#[test]
fn connect_detail_original_close_keeps_history_without_new_io_or_scope(){
    let who=owner();let scope=RpcConnectionScope::fresh();let a=ConnectAttempt::original(&who,&scope);a.entering(ConnectFailureStep::Dns);scope.close();a.finish(ConnectError::Timeout);assert_eq!(scope.connect_timeout_for(&who),Some(ConnectFailureStep::Dns));assert!(!scope.is_open());assert!(!scope.session_revoked());
}
#[test]
fn connect_detail_does_not_refund_or_reset_original_rpc_reservation(){
    let who=owner();let scope=RpcConnectionScope::fresh();let held=scope.reserve_native_rpc(100).unwrap();let last=held.clone();scope.record_connect_timeout(&who,ConnectFailureStep::Dns);assert_eq!(scope.native_rpc_resources(),(1,100));scope.close();drop(held);assert_eq!(scope.native_rpc_resources(),(1,100));drop(last);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn connect_detail_independent_slot_does_not_reenter_nonblocking_io_gate(){
    let who=owner();let scope=RpcConnectionScope::fresh();assert_eq!(scope.with_io_gate(||{scope.record_connect_timeout(&who,ConnectFailureStep::Tcp);9}),Some(9));assert!(scope.is_open());assert_eq!(scope.connect_timeout_for(&who),Some(ConnectFailureStep::Tcp));
}
#[test]
fn connect_detail_concurrent_followups_preserve_original_first_step(){
    let who=owner();let store=Arc::new(ConnectDiagnosticStore::original());store.record(&who,ConnectFailureStep::Dns);let jobs=(0..2).map(|_|{let s=store.clone();let w=who.clone();std::thread::spawn(move||for _ in 0..50{s.record(&w,ConnectFailureStep::Tcp);})}).collect::<Vec<_>>();for j in jobs{j.join().unwrap();}assert_eq!(store.for_owner(&who),Some(ConnectFailureStep::Dns));
}
#[test]
fn connect_detail_and_tls_reason_are_independent_data_slots_not_trust(){
    let who=owner();let scope=RpcConnectionScope::fresh();scope.record_connect_timeout(&who,ConnectFailureStep::Dns);scope.record_tls_detail(&who,n::tls_failure_detail::TlsFailureReason::NameMismatch);assert_eq!(scope.connect_timeout_for(&who),Some(ConnectFailureStep::Dns));assert_eq!(scope.tls_detail_for(&who),Some(n::tls_failure_detail::TlsFailureReason::NameMismatch));assert_eq!(scope.native_rpc_resources(),(0,0));assert!(scope.is_open());
}
