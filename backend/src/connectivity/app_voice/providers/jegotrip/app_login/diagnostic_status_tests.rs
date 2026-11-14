//! Actions-only checks: retain the existing diagnostic response, never a grant.
use super::*;
use super::super::{device_sync::DeviceSyncObservation, device_trust::TrustQueryState};

fn status_body(authentication:Value,forward:&str,local:Value)->Value {
    json!({"code":0,"body":{"authentication_code":authentication,"cs_forward":forward,"localCsForward":local,
        "token":"must-not-export-response-token","device_id":"must-not-export-response-device"}})
}
fn diagnostic_steps(authentication:Value,forward:&str,local:Value)->Vec<Step> {
    let mut steps=app_only_steps();
    steps.push(reply("/security",json!({"code":0,"body":{"authentication_code":2}})));
    steps.push(reply("/trust",json!({"code":0,"body":{"authentication_code":1}})));
    steps.push(reply("/state",status_body(authentication,forward,local)));steps
}
fn captured(status:Result<http::CallStatus,Error>)->DeviceTrustObservation {
    let snapshot=device_identity::Snapshot {id:id(),device_id:Secret::from_owned("PRIVATE-PHONE-UUID".into()),revision:4};
    DeviceTrustObservation::capture(&snapshot,100,
        Ok(Some(http::TrustDecision {authentication_code:Some(2)})),
        Ok(Some(http::TrustDecision {authentication_code:Some(1)})),status.map(|value|value.hosting_enabled))
        .with_diagnostic_status(status)
}
fn observed()->DeviceTrustObservation {
    captured(Ok(http::CallStatus {authentication_code:Some(2),hosting_enabled:true,local_cs_forward:Some(1)}))
}
async fn assert_diagnostic_only(f:&Fixture,view:&AttemptView) {
    assert_eq!(view.phase,"completed");assert_eq!(view.intent,LoginIntent::CheckDeviceTrust);
    assert_eq!(view.business_state,"not_requested");assert!(view.line_id.is_none());assert_redacted(view);
    assert_eq!(f.script.paths(),["/login","/user","/security","/trust","/state"]);
    assert_eq!(f.factory.build_count(),0);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());
    assert_eq!(device_rows(f,"app_voice_runtime_leases"),0);
}

#[tokio::test]
async fn diagnostic_preserves_conflicting_current_status_without_promoting_existing_device_success() {
    let f=Fixture::new(diagnostic_steps(json!(2),"2",json!(1)),true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_diagnostic_only(&f,&view).await;
    let observation=view.device_trust.as_ref().unwrap();
    assert_eq!(observation.security_verify.authentication_code,Some(2));
    assert_eq!(observation.existing_device.authentication_code,Some(1));
    assert_eq!(observation.call_status.as_ref().unwrap().authentication_code,Some(2));
    assert_eq!(observation.call_status.as_ref().unwrap().state,TrustQueryState::Observed);
    assert_eq!(observation.hosting_enabled,Some(true));assert_eq!(observation.local_cs_forward,Some(1));
    assert!(observation.device_sync.is_none());
    let encoded=serde_json::to_string(&view).unwrap();
    assert!(!encoded.contains("must-not-export"));assert!(!encoded.contains("PRIVATE-PHONE-UUID"));
    let readback=f.manager.get(&attempt).unwrap().device_trust.unwrap();
    assert_eq!(readback.call_status.unwrap().authentication_code,Some(2));assert_eq!(readback.local_cs_forward,Some(1));
    let seen=lock(&f.script.seen);
    assert!(seen[0].body.get("enrollId").is_none());
    assert_eq!(seen[4].body,json!({"device_id":"D5C0199B-1644-4D33-8912-B8BD5AB49224"}));
    assert_eq!(seen[4].wire,seen[4].body,"same single ordinary status request, no extra encrypted probe");
    for request in seen.iter().skip(1) {assert_eq!(request.query["token"],"fixture-app-session");}
    drop(seen);f.manager.shutdown().await;
}

#[tokio::test]
async fn diagnostic_positive_code_with_disabled_business_remains_metadata_only() {
    let f=Fixture::new(diagnostic_steps(json!("1"),"1",json!("2")),false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_diagnostic_only(&f,&view).await;
    let observation=view.device_trust.unwrap();
    assert_eq!(observation.call_status.unwrap().authentication_code,Some(1));
    assert_eq!(observation.hosting_enabled,Some(false));assert_eq!(observation.local_cs_forward,Some(2));
    f.manager.shutdown().await;
}

#[tokio::test]
async fn diagnostic_status_transport_failure_keeps_both_fields_unknown_without_retry() {
    let mut steps=diagnostic_steps(json!(1),"2",json!(2));
    *steps.last_mut().unwrap()=Step::Fail("/state".into());
    let f=Fixture::new(steps,false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_diagnostic_only(&f,&view).await;
    let observation=view.device_trust.unwrap();let status=observation.call_status.unwrap();
    assert_eq!(status.state,TrustQueryState::Failed);assert_eq!(status.authentication_code,None);
    assert_eq!(status.error.as_deref(),Some("app_login_transport_failed"));
    assert_eq!(observation.business_query_error,status.error);assert_eq!(observation.business_query_server_code,status.server_code);
    assert_eq!(observation.hosting_enabled,None);assert_eq!(observation.local_cs_forward,None);
    f.manager.shutdown().await;
}

#[tokio::test]
async fn diagnostic_invalid_business_response_cannot_preserve_an_apparent_positive_authentication() {
    let f=Fixture::new(diagnostic_steps(json!(1),"invalid",json!(2)),true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_diagnostic_only(&f,&view).await;
    let observation=view.device_trust.unwrap();let status=observation.call_status.unwrap();
    assert_eq!(status.state,TrustQueryState::Failed);assert_eq!(status.authentication_code,None);
    assert_eq!(status.error.as_deref(),Some("app_login_response_invalid"));
    assert_eq!(observation.business_query_error,status.error);assert_eq!(observation.hosting_enabled,None);
    assert_eq!(observation.local_cs_forward,None);f.manager.shutdown().await;
}

#[tokio::test]
async fn stored_positive_diagnostic_cannot_replace_a_new_reuse_attempts_current_rejection() {
    let mut steps=diagnostic_steps(json!(1),"2",json!(2));
    steps.extend(app_only_steps());steps.push(reply("/state",status_body(json!(2),"1",json!(1))));
    let f=Fixture::new(steps,false);let mut configuration=database_configuration(&f,true);
    configuration.http.endpoints.security_verify=Some("security".into());
    configuration.http.endpoints.verification_trustful_device=Some("trust".into());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration}).unwrap();
    let first=id();f.manager.submit_jegotrip(trust_check_request(&first)).unwrap();
    let prior=terminal(&f.manager,&first).await;assert_diagnostic_only(&f,&prior).await;
    assert_eq!(prior.device_trust.as_ref().unwrap().call_status.as_ref().unwrap().authentication_code,Some(1));
    tokio::time::timeout(Duration::from_secs(4),async {
        while f.manager.active.available_permits()!=2 {tokio::task::yield_now().await;}
    }).await.unwrap();
    let second=id();let mut request=trust_check_request(&second);request.intent=LoginIntent::ReuseHosting;
    f.manager.submit_jegotrip(request).unwrap();let next=terminal(&f.manager,&second).await;
    assert_eq!(next.phase,"failed");assert_eq!(next.error,Some("app_login_device_untrusted"));
    assert_eq!(next.account_id,prior.account_id);assert!(next.line_id.is_none());assert_redacted(&next);
    let observation=next.device_trust.as_ref().unwrap();
    assert_eq!(observation.call_status.as_ref().unwrap().authentication_code,Some(2));
    assert_eq!(observation.hosting_enabled,Some(false));assert_eq!(observation.local_cs_forward,Some(1));
    assert_eq!(observation.security_verify.state,TrustQueryState::NotRequested);
    assert_eq!(observation.existing_device.state,TrustQueryState::NotRequested);assert!(observation.device_sync.is_some());
    assert_eq!(f.manager.get(&first).unwrap().device_trust.unwrap().call_status.unwrap().authentication_code,Some(1));
    assert_eq!(f.script.paths(),["/login","/user","/security","/trust","/state","/login","/user","/state"]);
    assert_eq!(f.factory.build_count(),0);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());
    assert_eq!(device_rows(&f,"app_voice_runtime_leases"),0);f.manager.shutdown().await;
}

#[test]
fn diagnostic_status_codec_retains_unknown_and_noncanonical_codes_without_an_invented_success() {
    for authentication in [Value::Null,json!("01"),json!("+1"),json!(true),json!(-1),json!(1000000),json!("9999999")] {
        let response=status_body(authentication,"2",json!("01"));
        let status=http::decode_call_status(response["body"].as_object().unwrap()).unwrap();
        let observation=captured(Ok(status));let encoded=observation.encode().unwrap();
        assert!(!encoded.contains("must-not-export"));let decoded=DeviceTrustObservation::decode(&encoded).unwrap();
        assert_eq!(decoded.call_status.unwrap().authentication_code,None);
        assert_eq!(decoded.local_cs_forward,None);assert_eq!(decoded.hosting_enabled,Some(true));
    }
}

#[test]
fn diagnostic_status_codec_preserves_legacy_records_without_backfilling_response_fields() {
    let snapshot=device_identity::Snapshot {id:id(),device_id:Secret::from_owned("PRIVATE-PHONE-UUID".into()),revision:1};
    let old=DeviceTrustObservation::capture(&snapshot,100,Ok(None),Ok(None),Ok(true));
    let text=old.encode().unwrap();assert!(!text.contains("call_status"));assert!(!text.contains("local_cs_forward"));
    let decoded=DeviceTrustObservation::decode(&text).unwrap();
    assert!(decoded.call_status.is_none());assert!(decoded.local_cs_forward.is_none());assert!(decoded.device_sync.is_none());
    assert_eq!(decoded.encode().unwrap(),text,"no migration/reinterpretation of old bool-only diagnostics");
    let new=observed();let encoded=new.encode().unwrap();assert!(!encoded.contains("PRIVATE-PHONE-UUID"));
    assert_eq!(DeviceTrustObservation::decode(&encoded).unwrap().encode().unwrap(),encoded);
}

#[test]
fn diagnostic_status_codec_rejects_mixed_reuse_and_diagnostic_shapes() {
    let original=observed();assert!(original.encode().is_ok());
    let mut invalid=original.clone();invalid.device_sync=Some(DeviceSyncObservation::not_configured());assert!(invalid.encode().is_err());
    let mut invalid=original.clone();invalid.security_verify=device_trust::TrustQueryObservation::not_requested();assert!(invalid.encode().is_err());
    let mut invalid=original.clone();invalid.existing_device=device_trust::TrustQueryObservation::not_requested();assert!(invalid.encode().is_err());
    let mut invalid=original.clone();invalid.call_status=None;assert!(invalid.encode().is_err());
    let mut invalid=original.clone();invalid.call_status=Some(device_trust::TrustQueryObservation::capture(Ok(None)));assert!(invalid.encode().is_err());
    let mut invalid=original;invalid.call_status=Some(device_trust::TrustQueryObservation::not_requested());assert!(invalid.encode().is_err());
}

#[test]
fn diagnostic_status_codec_rejects_failure_and_business_metadata_inconsistency() {
    let failed=captured(Err(Error {code:"app_login_server_rejected",server_code:Some(506)}));
    assert!(failed.encode().is_ok());
    let mut invalid=failed.clone();invalid.hosting_enabled=Some(false);assert!(invalid.encode().is_err());
    let mut invalid=failed.clone();invalid.local_cs_forward=Some(1);assert!(invalid.encode().is_err());
    let mut invalid=failed.clone();invalid.business_query_error=Some("app_login_transport_failed".into());assert!(invalid.encode().is_err());
    let mut invalid=failed;invalid.business_query_server_code=Some(524);assert!(invalid.encode().is_err());
    let mut invalid=observed();invalid.hosting_enabled=None;assert!(invalid.encode().is_err());
    let mut invalid=observed();invalid.business_query_error=Some("app_login_transport_failed".into());assert!(invalid.encode().is_err());
}

#[test]
fn diagnostic_status_codec_keeps_original_numeric_and_two_kib_bounds() {
    let maximum=captured(Ok(http::CallStatus {authentication_code:Some(999999),hosting_enabled:false,local_cs_forward:Some(999999)}));
    let encoded=maximum.encode().unwrap();assert!(encoded.len()<=device_trust::MAX_OBSERVATION_BYTES);
    for bad in [-1,1000000,i64::MAX] {
        let mut invalid=maximum.clone();invalid.call_status.as_mut().unwrap().authentication_code=Some(bad);assert!(invalid.encode().is_err());
        let mut invalid=maximum.clone();invalid.local_cs_forward=Some(bad);assert!(invalid.encode().is_err());
    }
    assert!(DeviceTrustObservation::decode(&"x".repeat(2049)).is_err());
    let mut reflected:Value=serde_json::from_str(&encoded).unwrap();reflected["call_status"]["token"]=json!("not-a-grant");
    assert!(DeviceTrustObservation::decode(&reflected.to_string()).is_err());
}

#[test]
fn diagnostic_status_storage_is_owner_guarded_and_does_not_rewrite_a_finished_attempt() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",10).unwrap();let attempt=id();
    store::insert_with_intent(&conn,&attempt,"owner",LoginIntent::CheckDeviceTrust,10).unwrap();
    let observation=observed();assert!(store::set_device_trust(&conn,&attempt,"other-owner",&observation,100).is_err());
    store::set_device_trust(&conn,&attempt,"owner",&observation,100).unwrap();
    let readback=store::get(&conn,&attempt).unwrap().unwrap();assert_eq!(readback.business_state,"not_requested");
    assert!(readback.line_id.is_none());assert_eq!(readback.device_trust.unwrap().call_status.unwrap().authentication_code,Some(2));
    store::finish(&conn,&attempt,"owner",None,101).unwrap();
    let next=captured(Ok(http::CallStatus {authentication_code:Some(1),hosting_enabled:true,local_cs_forward:Some(2)}));
    assert!(store::set_device_trust(&conn,&attempt,"owner",&next,102).is_err());
    assert_eq!(store::get(&conn,&attempt).unwrap().unwrap().device_trust.unwrap().call_status.unwrap().authentication_code,Some(2));
}

#[tokio::test]
async fn diagnostic_status_storage_failure_cannot_create_a_native_account_or_repeat_the_query() {
    let f=Fixture::new(diagnostic_steps(json!(1),"2",json!(2)),false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    f.temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fixture_reject_diagnostic BEFORE UPDATE OF device_trust ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;")
        .map_err(|_|"fixture")).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.phase,"failed");assert_eq!(view.error,Some("app_login_storage_failed"));assert!(view.device_trust.is_none());
    assert_eq!(view.business_state,"not_requested");assert!(view.line_id.is_none());assert_redacted(&view);
    assert_eq!(f.script.paths(),["/login","/user","/security","/trust","/state"]);
    assert_eq!(f.factory.build_count(),0);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());
    assert_eq!(device_rows(&f,"app_voice_runtime_leases"),0);f.manager.shutdown().await;
}
