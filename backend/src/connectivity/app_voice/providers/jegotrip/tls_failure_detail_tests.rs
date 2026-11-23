//! SOURCE for original Actions only. Finite classifications, not TLS trust bypass.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use rustls::{CertificateError as C,Error as E,PeerIncompatible as P,pki_types::{ServerName,UnixTime}};
use std::{sync::Arc,time::Duration};
fn owner()->n::session::AttemptOwner{let mut s=n::session::HostingSession::new("fixture-tls-finite".into());s.import(n::tests::imported(),100).unwrap();s.begin_login(100).unwrap()}
#[test]
fn tls_detail_name_context_discards_all_presented_and_expected_private_names(){
    let error=E::InvalidCertificate(C::NotValidForNameContext{expected:ServerName::try_from("private-token.invalid".to_owned()).unwrap(),presented:vec!["private-uid.invalid".into(),"private-password.invalid".into()]});
    let value=TlsFailureReason::from_rustls(&error);assert_eq!(value,TlsFailureReason::NameMismatch);
    assert_eq!(serde_json::to_string(&value).unwrap(),"\"name_mismatch\"");assert!(!format!("{value:?}").contains("private-"));
}
#[test]
fn tls_detail_time_context_is_finite_and_does_not_export_timestamps(){
    let time=UnixTime::since_unix_epoch(Duration::from_secs(123456));
    for(error,want)in [(C::ExpiredContext{time,not_after:time},TlsFailureReason::Expired),(C::NotValidYetContext{time,not_before:time},TlsFailureReason::NotYetValid)]{
        let value=TlsFailureReason::from_rustls(&E::InvalidCertificate(error));assert_eq!(value,want);assert!(!serde_json::to_string(&value).unwrap().contains("123456"));
    }
}
#[test]
fn tls_detail_verification_reasons_do_not_conflate_name_ca_signature_or_purpose(){
    for(error,want)in [(C::BadEncoding,TlsFailureReason::BadEncoding),(C::UnknownIssuer,TlsFailureReason::UnknownIssuer),(C::Revoked,TlsFailureReason::Revoked),(C::BadSignature,TlsFailureReason::BadSignature),(C::NotValidForName,TlsFailureReason::NameMismatch),(C::InvalidPurpose,TlsFailureReason::InvalidPurpose),(C::UnhandledCriticalExtension,TlsFailureReason::InvalidChain)]{
        assert_eq!(TlsFailureReason::from_rustls(&E::InvalidCertificate(error)),want);
    }
}
#[test]
fn tls_detail_signature_context_does_not_copy_private_algorithm_lists(){
    let error=E::InvalidCertificate(C::UnsupportedSignatureAlgorithmForPublicKeyContext{signature_algorithm_id:b"private-token".to_vec(),public_key_algorithm_id:b"private-uid".to_vec()});
    let value=TlsFailureReason::from_rustls(&error);assert_eq!(value,TlsFailureReason::UnsupportedSignature);assert_eq!(serde_json::to_string(&value).unwrap(),"\"unsupported_signature\"");
}
#[test]
fn tls_detail_protocol_and_cipher_categories_never_enable_legacy_versions(){
    for(error,want)in [(P::ServerDoesNotSupportTls12Or13,TlsFailureReason::NoProtocolVersion),(P::ServerTlsVersionIsDisabledByOurConfig,TlsFailureReason::NoProtocolVersion),(P::NoCipherSuitesInCommon,TlsFailureReason::NoCipherSuite),(P::NoSignatureSchemesInCommon,TlsFailureReason::UnsupportedSignature)]{
        assert_eq!(TlsFailureReason::from_rustls(&E::PeerIncompatible(error)),want);
    }
}
#[test]
fn tls_detail_opaque_error_and_alert_data_never_become_formatted_public_reason(){
    assert_eq!(TlsFailureReason::from_rustls(&E::General("private-password private-token private-uid".into())),TlsFailureReason::ProtocolOther);
    assert_eq!(TlsFailureReason::from_rustls(&E::AlertReceived(rustls::AlertDescription::HandshakeFailure)),TlsFailureReason::PeerAlert);
    assert_eq!(TlsFailureReason::from_rustls(&E::DecryptError),TlsFailureReason::DecryptionFailed);
    assert_eq!(TlsFailureReason::from_rustls(&E::NoCertificatesPresented),TlsFailureReason::NoPeerChain);
}
#[test]
fn tls_detail_store_is_one_original_owner_slot_and_does_not_rewrite_first_cause(){
    let who=owner();let foreign=owner();let store=TlsDiagnosticStore::original();assert_eq!(store.for_owner(&who),None);
    store.record(&who,TlsFailureReason::NameMismatch);store.record(&who,TlsFailureReason::ProtocolOther);store.record(&foreign,TlsFailureReason::UnknownIssuer);
    assert_eq!(store.for_owner(&who),Some(TlsFailureReason::NameMismatch));assert_eq!(store.for_owner(&foreign),None);
}
#[test]
fn tls_detail_scope_clones_share_history_not_auth_or_independent_scope_data(){
    let who=owner();let scope=n::rpc_pending::RpcConnectionScope::fresh();let copy=scope.clone();let foreign=n::rpc_pending::RpcConnectionScope::fresh();
    scope.record_tls_detail(&who,TlsFailureReason::UnknownIssuer);assert_eq!(copy.tls_detail_for(&who),Some(TlsFailureReason::UnknownIssuer));assert_eq!(foreign.tls_detail_for(&who),None);
    assert_eq!(scope.native_rpc_resources(),(0,0));scope.close();assert_eq!(copy.tls_detail_for(&who),Some(TlsFailureReason::UnknownIssuer));assert!(!scope.is_open());assert!(!scope.session_revoked());
}
#[test]
fn tls_detail_does_not_reset_or_refund_original_shared_rpc_budget(){
    let who=owner();let scope=n::rpc_pending::RpcConnectionScope::fresh();let held=scope.reserve_native_rpc(100).unwrap();let last=held.clone();
    scope.record_tls_detail(&who,TlsFailureReason::BadSignature);assert_eq!(scope.native_rpc_resources(),(1,100));scope.close();drop(held);assert_eq!(scope.native_rpc_resources(),(1,100));drop(last);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn tls_detail_recording_is_independent_of_the_nonblocking_original_io_gate(){
    let who=owner();let scope=n::rpc_pending::RpcConnectionScope::fresh();
    assert_eq!(scope.with_io_gate(||{scope.record_tls_detail(&who,TlsFailureReason::PeerProtocol);7}),Some(7));
    assert_eq!(scope.tls_detail_for(&who),Some(TlsFailureReason::PeerProtocol));assert!(scope.is_open());
}
#[test]
fn tls_detail_concurrent_followups_preserve_first_owner_and_first_reason(){
    let who=owner();let store=Arc::new(TlsDiagnosticStore::original());store.record(&who,TlsFailureReason::NameMismatch);
    let jobs=(0..2).map(|_|{let store=store.clone();let who=who.clone();std::thread::spawn(move||for _ in 0..50{store.record(&who,TlsFailureReason::UnknownIssuer);})}).collect::<Vec<_>>();
    for job in jobs{job.join().unwrap();}assert_eq!(store.for_owner(&who),Some(TlsFailureReason::NameMismatch));
}
#[test]
fn tls_detail_legacy_tls_error_code_and_sdk_rejection_semantics_remain_exact(){
    assert_eq!(n::tls_engine::TlsError::HandshakeFailed.code(),"jegotrip_tls_handshake_failed");assert_eq!(n::tls_engine::TlsError::Timeout.code(),"jegotrip_tls_deadline_expired");
    let value=TlsFailureReason::ExactNameMissing;assert_eq!(serde_json::to_string(&value).unwrap(),"\"exact_name_missing\"");
}
