//! Actions-only original-UUID local receiver preparation, not remote forwarding.
use super::*;
use super::super::device_trust::{DeviceTrustPurpose,TrustQueryState};
use super::super::super::{rpc_logout,transport_packets};
const UUID:&str="D5C0199B-1644-4D33-8912-B8BD5AB49224";
const OTHER_UUID:&str="F1C0199B-1644-4D33-8912-B8BD5AB49224";

fn request(attempt:&str)->JegotripLoginRequest {
    let mut value=app_only_request(attempt);value.intent=LoginIntent::PrepareLocalReceiver;
    value.device_id=Some(Secret::from_owned(UUID.into()));value
}
fn configuration(f:&Fixture)->DatabaseConfiguration {
    let mut value=database_configuration(f,true);
    value.http.endpoints.verification_trustful_device=Some("trust".into());
    value.http.endpoints.device_enroll=Some("enroll".into());value.http.endpoints.security_verify=Some("security".into());
    value.http.enroll_id=Some(Secret::from_owned("fixture-stale-enrolment-not-for-local".into()));
    value.http.endpoints.device_sync=Some("device-sync".into());
    value.http.device_sync_profile=Some(serde_json::from_value(json!({"app_version":"8.9.4.0","app_version_code":8940,
        "device_type":"fixture-phone","os_version":"fixture-os","channel_id":"fixture-channel","terminal_brand":"fixture-brand"})).unwrap());
    value
}
fn configure(f:&Fixture) {f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:configuration(f)}).unwrap();}
fn recognized(code:Value)->Step {reply("/trust",json!({"code":0,"body":{"authentication_code":code}}))}
fn forwarding(code:Value,enabled:bool)->Step {reply("/state",json!({"code":0,"body":{
    "authentication_code":code,"cs_forward":if enabled {"2"}else{"1"},"localCsForward":"1","token":"must-not-export-forwarding-token"}}))}
fn steps()->Vec<Step> {
    let mut values=app_only_steps();values.push(recognized(json!(1)));
    values.push(reply("/csb/token",json!({"code":0,"body":{"token":"fixture-original-pwd"}})));
    values.push(reply("/voip",json!({"code":0,"body":{"sip_password":SIP_CIPHER,"status":"0"}})));
    values.push(recognized(json!("1")));values.push(forwarding(json!(2),false));values
}
async fn idle(f:&Fixture) {tokio::time::timeout(Duration::from_secs(4),async {
    while f.manager.active.available_permits()!=2 {tokio::task::yield_now().await;}
}).await.unwrap();}
async fn no_native(f:&Fixture,view:&AttemptView) {
    assert!(view.line_id.is_none());assert_eq!(f.factory.build_count(),0);
    assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());assert_redacted(view);
    assert_eq!(device_rows(f,"app_voice_runtime_leases"),0);
    assert_eq!(view.business_state,"not_requested");
}
fn observation(status:Option<Result<http::CallStatus,Error>>)->DeviceTrustObservation {
    let snapshot=device_identity::Snapshot {id:id(),device_id:Secret::from_owned("PRIVATE-PHONE-UUID".into()),revision:4};
    DeviceTrustObservation::capture_local_receiver(&snapshot,100,Ok(Some(http::TrustDecision {authentication_code:Some(1)})),status)
}

#[test]
fn local_receiver_is_explicit_requires_consent_original_uuid_and_keeps_all_old_defaults() {
    let f=Fixture::new(vec![],false);configure(&f);
    assert_eq!(LoginIntent::default(),LoginIntent::EnableHosting);
    assert_eq!(LoginIntent::parse("prepare_local_receiver"),Some(LoginIntent::PrepareLocalReceiver));
    assert!(!LoginIntent::PrepareLocalReceiver.reuses_trusted_device());assert!(LoginIntent::PrepareLocalReceiver.requires_original_device());
    assert!(f.manager.setup().accepting_local_receiver);
    for device in [None,Some("fixture-sdk-id"),Some("0123456789abcdef0123456789abcdef"),Some("123456789012345"),Some("")] {
        let mut value=request(&id());value.device_id=device.map(|v|Secret::from_owned(v.into()));
        assert_eq!(f.manager.submit_jegotrip(value).unwrap_err().code,"app_login_request_invalid");
    }
    let mut value=request(&id());value.consent=false;
    assert_eq!(f.manager.submit_jegotrip(value).unwrap_err().code,"app_login_request_invalid");
    assert_eq!(device_rows(&f,"jegotrip_device_identities"),0);assert!(f.script.paths().is_empty());
}

#[test]
fn local_receiver_missing_native_or_existing_device_endpoint_fails_before_login_or_identity_creation() {
    for native in [false,true] {
        let f=Fixture::new(vec![],false);let mut value=configuration(&f);
        if native {value.http.endpoints.verification_trustful_device=None;} else {value.runtime=None;}
        f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:value}).unwrap();
        assert!(!f.manager.configuration().unwrap().accepting_local_receiver);assert!(!f.manager.setup().accepting_local_receiver);
        assert_eq!(f.manager.submit_jegotrip(request(&id())).unwrap_err().code,
            if native {"app_login_receiver_device_check_missing"}else{"app_login_native_configuration_missing"});
        assert_eq!(device_rows(&f,"jegotrip_device_identities"),0);assert!(f.script.paths().is_empty());
    }
}

#[test]
fn local_receiver_rejects_imported_observations_tokens_or_report_profiles_in_the_request() {
    for field in ["device_trust","purpose","trust_granted","sso_token","phone_login_token","enrollId","device_sync_profile","sdk_device_id"] {
        let mut value=json!({"attempt_id":id(),"username":"user@example.invalid","password":" p@ss ","consent":true,
            "intent":"prepare_local_receiver","device_id":UUID,"start_receiver":false});value[field]=json!("must-not-import");
        assert!(serde_json::from_value::<JegotripLoginRequest>(value).is_err(),"{field}");
    }
}

#[tokio::test]
async fn local_receiver_prepares_without_forwarding_or_reports_and_retains_current_sdk_credentials_separately() {
    let f=Fixture::new(steps(),true);configure(&f);let attempt=id();
    f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.phase,"completed");assert_eq!(view.intent,LoginIntent::PrepareLocalReceiver);assert_eq!(view.business_state,"not_requested");assert_redacted(&view);
    let observation=view.device_trust.as_ref().unwrap();assert_eq!(observation.purpose,Some(DeviceTrustPurpose::LocalReceiver));
    assert_eq!(observation.existing_device.authentication_code,Some(1));assert_eq!(observation.security_verify.state,TrustQueryState::NotRequested);
    assert_eq!(observation.call_status.as_ref().unwrap().authentication_code,Some(2));assert_eq!(observation.hosting_enabled,Some(false));
    assert_eq!(observation.local_cs_forward,Some(1));assert!(observation.device_sync.is_none());
    let line=view.line_id.as_ref().unwrap();let account=f.accounts.get(line,now() as u64).await.unwrap();
    assert_eq!(account.phase,"stopped");assert!(!account.incoming_message_ready);assert!(!account.incoming_call_ready);assert!(!account.outbound_authorized);
    assert_eq!(f.factory.build_count(),0);assert_eq!(device_rows(&f,"app_voice_runtime_leases"),0);
    assert_eq!(f.script.paths(),["/login","/user","/trust","/csb/token","/voip","/trust","/state"]);
    let seen=lock(&f.script.seen);assert!(seen[0].body.get("enrollId").is_none());
    assert_eq!(seen[3].body,json!({"accountId":"+447700900123"}));
    for index in [2,5] {
        assert_eq!(seen[index].body,json!({"sso_token":"","equipment_type":"1","device_id":UUID}));
        assert_eq!(seen[index].wire,seen[index].body);
    }
    assert_eq!(seen[6].body,json!({"device_id":UUID}));assert_eq!(seen[6].wire,seen[6].body);
    for record in seen.iter().skip(1) {assert_eq!(record.query["token"],"fixture-app-session");}
    drop(seen);assert!(!serde_json::to_string(&view).unwrap().contains("must-not-export"));
    let readback=f.manager.get(&attempt).unwrap();assert_eq!(readback.device_trust.unwrap().purpose,Some(DeviceTrustPurpose::LocalReceiver));
    f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_treats_valid_forwarding_codes_and_switches_as_data_not_local_sdk_grants() {
    for (code,enabled) in [(json!(1),false),(json!(2),false),(json!(2),true),(Value::Null,true)] {
        let mut sequence=steps();*sequence.last_mut().unwrap()=forwarding(code.clone(),enabled);
        let f=Fixture::new(sequence,false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
        let view=terminal(&f.manager,&attempt).await;assert_eq!(view.phase,"completed");assert_eq!(view.business_state,"not_requested");
        let observation=view.device_trust.unwrap();assert_eq!(observation.call_status.unwrap().authentication_code,code.as_i64());
        assert_eq!(observation.hosting_enabled,Some(enabled));assert_eq!(f.factory.build_count(),0);
        let account=f.accounts.get(view.line_id.as_deref().unwrap(),now() as u64).await.unwrap();
        assert_eq!(account.phase,"stopped");assert!(!account.incoming_message_ready);assert!(!account.incoming_call_ready);
        f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_requires_exact_current_existing_device_one_before_any_phone_material() {
    for code in [json!(2),json!("01"),json!(true),Value::Null,json!(999999999)] {
        let mut sequence=app_only_steps();sequence.push(recognized(code));
        let f=Fixture::new(sequence,false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
        let view=terminal(&f.manager,&attempt).await;assert_eq!(view.error,Some("app_login_device_untrusted"));no_native(&f,&view).await;
        let obs=view.device_trust.unwrap();assert_eq!(obs.call_status.unwrap().state,TrustQueryState::NotRequested);
        assert_eq!(obs.hosting_enabled,None);assert_eq!(f.script.paths(),["/login","/user","/trust"]);f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_existing_device_transport_and_server_refusals_do_not_fallback_or_retry() {
    for (step,expected) in [(Step::Fail("/trust".into()),"app_login_transport_failed"),
        (reply("/trust",json!({"code":506,"body":{"authentication_code":1}})),"app_login_server_rejected")] {
        let mut sequence=app_only_steps();sequence.push(step);let f=Fixture::new(sequence,true);configure(&f);
        let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
        assert_eq!(view.error,Some(expected));no_native(&f,&view).await;
        assert_eq!(view.device_trust.as_ref().unwrap().existing_device.state,TrustQueryState::Failed);
        assert_eq!(f.script.paths(),["/login","/user","/trust"]);f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_rechecks_device_after_phone_material_and_stops_on_a_current_change() {
    let mut sequence=steps();sequence.truncate(6);sequence[5]=recognized(json!(2));
    let f=Fixture::new(sequence,false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_eq!(view.error,Some("app_login_device_untrusted"));no_native(&f,&view).await;
    let obs=view.device_trust.unwrap();assert_eq!(obs.existing_device.authentication_code,Some(2));assert_eq!(obs.call_status.unwrap().state,TrustQueryState::NotRequested);
    assert_eq!(f.script.paths(),["/login","/user","/trust","/csb/token","/voip","/trust"]);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_phone_service_status_is_data_not_a_substitute_for_credentials_or_sdk_acceptance() {
    for status in [json!(0),json!(1),json!("00"),Value::Null] {
        let mut sequence=steps();
        sequence[4]=reply("/voip",json!({"code":0,"body":{"sip_password":SIP_CIPHER,"status":status}}));
        let f=Fixture::new(sequence,false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
        let view=terminal(&f.manager,&attempt).await;assert_eq!(view.phase,"completed");assert_eq!(view.business_state,"not_requested");assert_redacted(&view);
        let account=f.accounts.get(view.line_id.as_deref().unwrap(),now() as u64).await.unwrap();
        assert_eq!(account.phase,"stopped");assert!(!account.incoming_message_ready);assert!(!account.incoming_call_ready);
        assert_eq!(f.factory.build_count(),0);assert_eq!(f.script.paths(),["/login","/user","/trust","/csb/token","/voip","/trust","/state"]);
        f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_positive_service_metadata_cannot_replace_missing_or_invalid_phone_material() {
    for phone_token in [true,false] {
        let mut sequence=steps();
        if phone_token {
            sequence.truncate(4);sequence[3]=reply("/csb/token",json!({"code":0,"body":{"token":"","status":"0","authentication_code":1}}));
        } else {
            sequence.truncate(5);sequence[4]=reply("/voip",json!({"code":0,"body":{"sip_password":"","status":"0","authentication_code":1}}));
        }
        let f=Fixture::new(sequence,false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
        let view=terminal(&f.manager,&attempt).await;assert_eq!(view.error,Some("app_login_response_invalid"));no_native(&f,&view).await;
        assert_eq!(f.script.paths().len(),if phone_token {4}else{5});f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_malformed_or_failed_forwarding_read_still_prevents_creation() {
    for step in [Step::Fail("/state".into()),reply("/state",json!({"code":0,"body":{"cs_forward":"02","authentication_code":1}}))] {
        let mut sequence=steps();*sequence.last_mut().unwrap()=step;let f=Fixture::new(sequence,false);configure(&f);
        let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
        assert!(matches!(view.error,Some("app_login_transport_failed"|"app_login_response_invalid")));no_native(&f,&view).await;
        let obs=view.device_trust.unwrap();assert_eq!(obs.call_status.unwrap().state,TrustQueryState::Failed);assert_eq!(obs.hosting_enabled,None);
        assert_eq!(f.script.paths().len(),7);f.manager.shutdown().await;
    }
}

#[tokio::test]
async fn local_receiver_missing_receive_bindings_stops_before_fetching_phone_credentials() {
    let mut sequence=steps();sequence.truncate(3);let f=Fixture::new(sequence,false);let mut value=configuration(&f);
    let p=&mut value.runtime.as_mut().unwrap().profile;p.incoming=None;p.call_incoming=None;p.native_receivers=None;
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:value}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.error,Some("app_login_native_configuration_invalid"));no_native(&f,&view).await;
    assert_eq!(f.script.paths(),["/login","/user","/trust"]);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_invalid_native_template_never_reports_device_or_starts_a_native_actor() {
    let mut sequence=steps();sequence.truncate(5);let f=Fixture::new(sequence,false);let mut value=configuration(&f);
    value.runtime.as_mut().unwrap().sdk.application_key=Secret::from_owned(String::new());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:value}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.error,Some("app_login_native_configuration_invalid"));no_native(&f,&view).await;
    assert_eq!(f.script.paths(),["/login","/user","/trust","/csb/token","/voip"]);f.manager.shutdown().await;
}

#[tokio::test]
async fn old_positive_diagnostic_cannot_authorize_a_new_local_receiver_attempt() {
    let mut sequence=trust_check_steps(1,true);sequence.extend(app_only_steps());sequence.push(recognized(json!(2)));
    let f=Fixture::new(sequence,false);configure(&f);let first=id();f.manager.submit_jegotrip(trust_check_request(&first)).unwrap();
    let prior=terminal(&f.manager,&first).await;assert_eq!(prior.phase,"completed");idle(&f).await;
    let next=id();f.manager.submit_jegotrip(request(&next)).unwrap();let view=terminal(&f.manager,&next).await;
    assert_eq!(view.error,Some("app_login_device_untrusted"));assert_eq!(view.account_id,prior.account_id);no_native(&f,&view).await;
    assert_eq!(f.manager.get(&first).unwrap().device_trust.unwrap().existing_device.authentication_code,Some(1));
    assert_eq!(view.device_trust.unwrap().existing_device.authentication_code,Some(2));f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_cannot_replace_an_existing_accounts_original_uuid() {
    let f=Fixture::new(app_only_steps(),false);let mut initial=configuration(&f);
    // This prerequisite is a non-enrolling ordinary App authentication, not
    // the local receiver operation being tested after its canonical binding.
    initial.http.endpoints.device_enroll=None;initial.http.enroll_id=None;
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:initial}).unwrap();
    let first=id();let mut auth=app_only_request(&first);
    auth.device_id=Some(Secret::from_owned(UUID.into()));f.manager.submit_jegotrip(auth).unwrap();
    let view=terminal(&f.manager,&first).await;assert_eq!(view.phase,"completed");idle(&f).await;
    let mut next=request(&id());next.device_id=Some(Secret::from_owned(OTHER_UUID.into()));
    assert_eq!(f.manager.submit_jegotrip(next).unwrap_err().code,"app_login_device_identity_conflict");
    let identities=f.manager.device_identities().unwrap();assert_eq!(identities.identities.len(),1);assert_eq!(identities.identities[0].device_id,UUID);
    assert_eq!(f.script.paths(),["/login","/user"]);assert_eq!(device_rows(&f,"app_voice_accounts"),0);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_shutdown_during_recognition_keeps_unknown_status_and_releases_admission() {
    let mut sequence=app_only_steps();sequence.push(Step::Park("/trust".into()));let f=Fixture::new(sequence,false);configure(&f);
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async {while f.script.paths().len()<3 {tokio::task::yield_now().await;}}).await.unwrap();
    f.manager.shutdown().await;let view=f.manager.get(&attempt).unwrap();assert_eq!(view.error,Some("app_login_interrupted"));
    assert!(view.completed);no_native(&f,&view).await;assert_eq!(f.manager.active.available_permits(),2);
    assert_eq!(f.script.paths(),["/login","/user","/trust"]);
}

#[tokio::test]
async fn local_receiver_cannot_start_a_second_same_alias_job_while_the_first_is_waiting() {
    let mut sequence=app_only_steps();sequence.push(Step::Park("/trust".into()));let f=Fixture::new(sequence,false);configure(&f);
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async {while f.script.paths().len()<3 {tokio::task::yield_now().await;}}).await.unwrap();
    assert_eq!(f.manager.submit_jegotrip(request(&id())).unwrap_err().code,"app_login_busy");
    assert_eq!(device_rows(&f,"jegotrip_login_attempts"),1);f.manager.shutdown().await;
    assert_eq!(f.manager.active.available_permits(),2);assert_eq!(f.script.paths().len(),3);
}

#[tokio::test]
async fn local_receiver_http_policy_forbids_enrollment_business_and_device_report_even_with_positive_data() {
    let f=Fixture::new(app_only_steps(),false);let value=configuration(&f);
    let mut client=Factory(f.script.clone()).build(&value.http).unwrap().with_device_id(UUID).with_local_receiver().unwrap();
    let input=login_request(&id());client.ensure_device_enrollment().await;client.login(&input).await.unwrap();
    let bound=client.bound_number(&input).await.unwrap();
    assert_eq!(client.enroll_device().await.unwrap_err().code,"app_login_request_invalid");
    assert_eq!(client.verify_trusted_device().await.unwrap_err().code,"app_login_request_invalid");
    assert_eq!(client.phone_credentials(&bound).await.err().unwrap().code,"app_login_request_invalid");
    assert_eq!(client.enable_business().await.unwrap_err().code,"app_login_request_invalid");
    assert_eq!(client.sync_device(&bound,http::CallStatus {authentication_code:Some(1),hosting_enabled:true,local_cs_forward:Some(2)}).await.unwrap_err().code,"app_login_request_invalid");
    assert!(!client.trusted_reuse_write_started());assert_eq!(f.script.paths(),["/login","/user"]);
    assert!(lock(&f.script.seen)[0].body.get("enrollId").is_none());f.manager.shutdown().await;
}

#[tokio::test]
async fn ordinary_and_remote_reuse_clients_cannot_invoke_local_receiver_only_methods() {
    for reusing in [false,true] {
        let f=Fixture::new(app_only_steps(),false);let value=configuration(&f);
        let mut client=Factory(f.script.clone()).build(&value.http).unwrap().with_device_id(UUID);
        if reusing {client=client.with_trusted_reuse().unwrap();}
        let input=login_request(&id());client.login(&input).await.unwrap();let bound=client.bound_number(&input).await.unwrap();
        assert_eq!(client.receiver_device_recognition().await.unwrap_err().code,"app_login_request_invalid");
        assert_eq!(client.receiver_phone_credentials(&bound).await.err().unwrap().code,"app_login_request_invalid");
        assert!(client.with_local_receiver().is_err());assert_eq!(f.script.paths(),["/login","/user"]);f.manager.shutdown().await;
    }
}

#[test]
fn local_receiver_observation_roundtrip_is_distinct_and_does_not_backfill_legacy_rows() {
    let value=observation(Some(Ok(http::CallStatus {authentication_code:Some(2),hosting_enabled:false,local_cs_forward:Some(1)})));
    let encoded=value.encode().unwrap();assert!(!encoded.contains("PRIVATE-PHONE-UUID"));assert!(encoded.contains("local_receiver"));
    let restored=DeviceTrustObservation::decode(&encoded).unwrap();assert_eq!(restored.encode().unwrap(),encoded);
    assert_eq!(restored.purpose,Some(DeviceTrustPurpose::LocalReceiver));assert_eq!(restored.existing_device.authentication_code,Some(1));
    assert_eq!(restored.call_status.unwrap().authentication_code,Some(2));assert_eq!(restored.hosting_enabled,Some(false));
    let mut legacy=value;legacy.purpose=None;legacy.security_verify=device_trust::TrustQueryObservation::capture(Ok(None));
    let text=legacy.encode().unwrap();assert!(!text.contains("purpose"));assert!(DeviceTrustObservation::decode(&text).unwrap().purpose.is_none());
}

#[test]
fn local_receiver_observation_preserves_unrequested_and_failed_forwarding_without_fabricating_closed_or_open() {
    let first=observation(None);assert_eq!(first.hosting_enabled,None);assert_eq!(first.business_query_error,None);
    assert_eq!(first.call_status.as_ref().unwrap().state,TrustQueryState::NotRequested);assert!(first.encode().is_ok());
    let failed=observation(Some(Err(Error::new("app_login_transport_failed"))));assert!(failed.encode().is_ok());
    assert_eq!(failed.hosting_enabled,None);assert_eq!(failed.local_cs_forward,None);
    assert_eq!(failed.call_status.unwrap().error,failed.business_query_error);
}

#[test]
fn local_receiver_observation_rejects_mixed_report_query_purpose_and_business_shapes() {
    let original=observation(None);
    let mut bad=original.clone();bad.purpose=None;assert!(bad.encode().is_err());
    let mut bad=original.clone();bad.device_sync=Some(DeviceSyncObservation::not_requested());assert!(bad.encode().is_err());
    let mut bad=original.clone();bad.security_verify=device_trust::TrustQueryObservation::capture(Ok(None));assert!(bad.encode().is_err());
    let mut bad=original.clone();bad.existing_device=device_trust::TrustQueryObservation::not_requested();assert!(bad.encode().is_err());
    let mut bad=original.clone();bad.call_status=None;assert!(bad.encode().is_err());
    let mut bad=original;bad.hosting_enabled=Some(false);assert!(bad.encode().is_err());
    let mut bad=observation(Some(Err(Error::new("app_login_transport_failed"))));bad.business_query_server_code=Some(506);assert!(bad.encode().is_err());
}

#[test]
fn local_receiver_observation_retains_numeric_size_and_unknown_field_rejection() {
    let original=observation(Some(Ok(http::CallStatus {authentication_code:Some(999999),hosting_enabled:true,local_cs_forward:Some(999999)})));
    assert!(original.encode().unwrap().len()<2048);
    for bad in [-1,1000000,i64::MAX] {
        let mut value=original.clone();value.local_cs_forward=Some(bad);assert!(value.encode().is_err());
        let mut value=original.clone();value.existing_device.authentication_code=Some(bad);assert!(value.encode().is_err());
    }
    let mut value:Value=serde_json::from_str(&original.encode().unwrap()).unwrap();value["purpose"]=json!("forged_ready");
    assert!(DeviceTrustObservation::decode(&value.to_string()).is_err());value["purpose"]=json!("local_receiver");value["token"]=json!("secret");
    assert!(DeviceTrustObservation::decode(&value.to_string()).is_err());assert!(DeviceTrustObservation::decode(&"x".repeat(2049)).is_err());
}

#[tokio::test]
async fn local_receiver_early_observation_storage_failure_stops_before_phone_material() {
    let mut sequence=steps();sequence.truncate(3);let f=Fixture::new(sequence,false);configure(&f);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fixture_local_observation BEFORE UPDATE OF device_trust ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.error,Some("app_login_storage_failed"));no_native(&f,&view).await;
    assert!(view.device_trust.is_none());assert_eq!(f.script.paths(),["/login","/user","/trust"]);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_final_observation_storage_failure_stops_before_account_creation() {
    let f=Fixture::new(steps(),false);configure(&f);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fixture_local_final BEFORE UPDATE OF device_trust ON jegotrip_login_attempts WHEN json_extract(NEW.device_trust,'$.call_status.state')='observed' BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.error,Some("app_login_storage_failed"));no_native(&f,&view).await;
    assert_eq!(view.device_trust.unwrap().call_status.unwrap().state,TrustQueryState::NotRequested);
    assert_eq!(f.script.paths().len(),7);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_reusing_a_completed_request_id_reads_existing_state_without_resubmission() {
    let f=Fixture::new(steps(),false);configure(&f);let attempt=id();f.manager.submit_jegotrip(request(&attempt)).unwrap();
    let first=terminal(&f.manager,&attempt).await;assert_eq!(first.phase,"completed");idle(&f).await;
    assert_eq!(f.manager.submit_jegotrip(request(&attempt)).unwrap_err().code,"app_login_attempt_exists");
    assert_eq!(f.manager.get(&attempt).unwrap().line_id,first.line_id);assert_eq!(f.script.paths().len(),7);
    assert_eq!(device_rows(&f,"app_voice_accounts"),1);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_native_login_registration_sms_ack_and_stop_join_do_not_enable_forwarding() {
    let f=Fixture::new(steps(),false);configure(&f);let (mut native,_,dropped)=peer(&f.cert,&f.factory);
    let serving=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(12),async move {
        native.authenticate_as(b"[username:+447700900123@fixture.invalid]",b"fixture-original-pwd").await;
        native.swap(Some(4),true).await;
        native.incoming_with_fields(93,b"Sip.Request",b"fixture-local-SMS",Some(b"fixture-sender"),None).await;
        assert_eq!(next_non_poll(&mut native).await.kind,transport_packets::CallKind::Reply);
        native.swap(Some(3),true).await;let message=next_non_poll(&mut native).await;
        let rpc=decode_request(message.payload.expose_to_transport()).unwrap();assert_eq!(rpc.method,rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        native.reply(rpc.call_id,&[0,1,0,0,0,0]).await;assert!(native.receive().await.is_none());
    }).await.unwrap()});
    let attempt=id();let mut input=request(&attempt);input.start_receiver=true;f.manager.submit_jegotrip(input).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_eq!(view.phase,"completed");assert_eq!(view.business_state,"not_requested");assert_redacted(&view);
    assert_eq!(view.device_trust.as_ref().unwrap().hosting_enabled,Some(false));let line=view.line_id.unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {loop {
        let account=f.accounts.get(&line,now() as u64).await.unwrap();
        if account.incoming_message_ready && f.accounts.messages(&line,10).unwrap().len()==1 {
            assert!(!account.incoming_call_ready);assert!(!account.outbound_authorized);break;
        }
        tokio::task::yield_now().await;
    }}).await.unwrap();
    assert_eq!(f.script.paths(),["/login","/user","/trust","/csb/token","/voip","/trust","/state"]);
    f.accounts.stop(&line,1,now() as u64).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(f.factory.build_count(),1);assert_eq!(device_rows(&f,"app_voice_runtime_leases"),0);f.manager.shutdown().await;
}

#[tokio::test]
async fn local_receiver_existing_device_and_forwarding_use_pinned_https_without_static_enrollment_or_new_carrier_token() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server=cert.server_config(&rustls::version::TLS13);
    let serving=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move {
        let mut requests=vec![];
        for body in [r#"{"code":0,"body":{"token":"fixture-owned-https-session"}}"#,r#"{"code":0,"body":{"authentication_code":"1"}}"#,
            r#"{"code":0,"body":{"cs_forward":"1","authentication_code":"2","localCsForward":"1"}}"#] {
            let (tcp,_)=listener.accept().await.unwrap();let mut stream=tokio_rustls::TlsAcceptor::from(server.clone()).accept(tcp).await.unwrap();
            let mut request=vec![];let mut bytes=[0u8;2048];
            loop {let n=stream.read(&mut bytes).await.unwrap();assert!(n>0);request.extend_from_slice(&bytes[..n]);assert!(request.len()<65536);
                if let Some(end)=request.windows(4).position(|w|w==b"\r\n\r\n") {
                    let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|v|v.trim().parse::<usize>().unwrap()).unwrap_or(0);
                    if request.len()>=end+4+length {break;}
                }
            }
            requests.push(request);let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            stream.write_all(response.as_bytes()).await.unwrap();let _=stream.shutdown().await;
        } requests
    }).await.unwrap()});
    let mut config=http_config(false);config.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));config.root_der_base64=None;
    config.pinned_server_certificate_der_base64=Some(Secret::from_owned(cert.root.clone()));config.pinned_server_public_key_der_base64=Some(Secret::from_owned(cert.public_key.clone()));
    config.enroll_id=Some(Secret::from_owned("fixture-stale-enrolment".into()));config.endpoints.verification_trustful_device=Some("trust".into());
    let mut client=LiveFactory.build(&config).unwrap().with_device_id(UUID).with_local_receiver().unwrap();client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.receiver_device_recognition().await.unwrap().unwrap().authentication_code,Some(1));
    let status=client.call_status().await.unwrap();assert_eq!(status.authentication_code,Some(2));assert!(!status.hosting_enabled);
    let requests=serving.await.unwrap();let first=String::from_utf8(requests[0].clone()).unwrap();assert!(!first.contains("enrollId"));assert!(!first.contains("p@ss"));
    let check=String::from_utf8(requests[1].clone()).unwrap();assert!(check.starts_with("POST /trust?lang=zh_CN&token=fixture-owned-https-session"));
    assert_eq!(serde_json::from_str::<Value>(check.split("\r\n\r\n").nth(1).unwrap()).unwrap(),json!({"sso_token":"","equipment_type":"1","device_id":UUID}));
    let forwarding=String::from_utf8(requests[2].clone()).unwrap();assert!(forwarding.starts_with("POST /state?lang=zh_CN&token=fixture-owned-https-session"));
    assert_eq!(serde_json::from_str::<Value>(forwarding.split("\r\n\r\n").nth(1).unwrap()).unwrap(),json!({"device_id":UUID}));
}
