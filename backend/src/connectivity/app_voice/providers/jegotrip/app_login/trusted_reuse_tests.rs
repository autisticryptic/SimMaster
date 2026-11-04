//! Actions-only original post-trust branches: synthetic phone context, no VTD.
use super::*;
use super::super::{device_identity as device, device_sync, http};
use super::super::super::{account_runs, rpc_logout, transport_packets};

const PHONE_UUID:&str="D5C0199B-1644-4D33-8912-B8BD5AB49224";
const OTHER_UUID:&str="F1C0199B-1644-4D33-8912-B8BD5AB49224";
fn reuse_request(attempt:&str,intent:LoginIntent)->JegotripLoginRequest {
    serde_json::from_value(json!({"attempt_id":attempt,"username":"user@example.invalid","password":" p@ss ","country_code":null,
        "intent":intent,"consent":true,"start_receiver":false,"device_id":PHONE_UUID})).unwrap()
}
fn sync_profile()->device_sync::DeviceSyncProfile {
    serde_json::from_value(json!({"app_version":"8.9.8.0","app_version_code":8980,"device_type":"fixture-phone-model",
        "os_version":"Android fixture-version","channel_id":"fixture-phone-channel","terminal_brand":"fixture-phone-brand"})).unwrap()
}
fn reuse_configuration(f:&Fixture,sync:bool)->DatabaseConfiguration {
    let mut c=database_configuration(f,true);
    c.http.endpoints.device_enroll=Some("enroll".into());c.http.endpoints.security_verify=Some("security".into());
    c.http.endpoints.verification_trustful_device=Some("trust".into());
    c.http.enroll_id=Some(Secret::from_owned("fixture-stale-other-device-enrolment".into()));
    if sync {c.http.endpoints.device_sync=Some("device-sync".into());c.http.device_sync_profile=Some(sync_profile());}
    c
}
fn owned_login_steps()->Vec<Step> {
    let mut s=app_only_steps();
    s[1]=reply("/user",json!({"code":0,"body":{"mobile":"7700900123","country_code":"44",
        "accountid":"fixture-owned-app-account-id","user_id":"must-not-substitute-user-id"}}));s
}
fn state(enabled:bool,auth:Value)->Step {
    reply("/state",json!({"code":0,"body":{"cs_forward":if enabled {"2"}else{"1"},"authentication_code":auth,"localCsForward":"2"}}))
}
fn reuse_steps(enabled:bool,sync:bool)->Vec<Step> {
    let mut s=owned_login_steps();s.push(state(enabled,json!("1")));
    s.extend(prefix().into_iter().skip(2));s.push(state(enabled,json!(1)));
    if sync {s.push(reply("/device-sync",json!({"code":0,"body":{"version":"ignored-upgrade","token":"must-not-retain-sync-token"}})));s.push(state(enabled,json!(1)));}
    s
}
fn configure_reuse(f:&Fixture,sync:bool) {f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:reuse_configuration(f,sync)}).unwrap();}
async fn idle(f:&Fixture) {tokio::time::timeout(Duration::from_secs(4),async {
    while f.manager.active.available_permits()!=2 {tokio::task::yield_now().await;}
}).await.unwrap();}
async fn assert_no_native(f:&Fixture,v:&AttemptView) {
    assert!(v.line_id.is_none());assert_eq!(f.factory.build_count(),0);
    assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());assert_redacted(v);
}
fn rows(f:&Fixture,table:&str)->i64 {device_rows(f,table)}

#[test]
fn trusted_reuse_requires_explicit_original_uuid_consent_and_native_context_without_an_auto_seed() {
    let f=Fixture::new(vec![],false);configure_reuse(&f,false);
    for intent in [LoginIntent::ReuseHosting,LoginIntent::EnableTrustedHosting] {
        for uuid in [None,Some("fixture-stale-device"),Some("0123456789abcdef0123456789abcdef"),Some("123456789012345"),Some("bad-uuid")] {
            let mut request=reuse_request(&id(),intent);request.device_id=uuid.map(|s|Secret::from_owned(s.into()));
            assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code,"app_login_request_invalid");
        }
        let mut request=reuse_request(&id(),intent);request.consent=false;
        assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code,"app_login_request_invalid");
    }
    assert_eq!(rows(&f,"jegotrip_device_identities"),0);assert_eq!(rows(&f,"jegotrip_login_attempts"),0);
    assert!(f.script.paths().is_empty());assert_eq!(f.factory.build_count(),0);
    let f=Fixture::new(vec![],false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&id(),LoginIntent::ReuseHosting)).unwrap_err().code,"app_login_native_configuration_missing");
    assert!(f.script.paths().is_empty());assert_eq!(rows(&f,"jegotrip_device_identities"),0);
    for field in ["trust_granted","sso_token","enrollId","userId","hardware_imei","device_sync_profile"] {
        let mut value=serde_json::to_value(json!({"attempt_id":id(),"username":"user@example.invalid","password":" p@ss ","intent":"reuse_hosting",
            "consent":true,"start_receiver":false,"device_id":PHONE_UUID})).unwrap();value[field]=json!("must-not-import");
        assert!(serde_json::from_value::<JegotripLoginRequest>(value).is_err());
    }
}
#[test]
fn original_phone_uuid_shape_does_not_accept_sdk_ids_imeis_or_md5_as_reuse_identity() {
    assert!(device::valid_phone_app_uuid(PHONE_UUID));assert!(device::valid_phone_app_uuid(&PHONE_UUID.to_ascii_lowercase()));
    for bad in ["", "PHONE-DEVICE-0001","0123456789abcdef0123456789abcdef","123456789012345","D5C0199B_1644-4D33-8912-B8BD5AB49224"] {
        assert!(!device::valid_phone_app_uuid(bad));
    }
}
#[test]
fn get_call_status_preserves_exact_authentication_code_and_camelcase_local_state_without_claiming_ready() {
    for (auth,expected) in [(json!("1"),Some(1)),(json!(1),Some(1)),(json!("2"),Some(2)),(json!("01"),None),(json!(true),None),(json!(null),None)] {
        let body=json!({"cs_forward":"2","authentication_code":auth,"localCsForward":"2","local_cs_forward":"999999999","token":"not-a-grant"});
        let status=http::decode_call_status(body.as_object().unwrap()).unwrap();
        assert_eq!(status.authentication_code,expected);assert!(status.hosting_enabled);assert_eq!(status.local_cs_forward,Some(2));
    }
    let body=json!({"cs_forward":"1","localCsForward":"02"});
    let status=http::decode_call_status(body.as_object().unwrap()).unwrap();assert_eq!(status.authentication_code,None);assert_eq!(status.local_cs_forward,None);
    for value in [json!("02"),json!(0),json!(true),json!(null)] {
        let body=json!({"cs_forward":value,"authentication_code":1});assert!(http::decode_call_status(body.as_object().unwrap()).is_err());
    }
}
#[tokio::test]
async fn original_uuid_reuse_checks_server_trust_before_credentials_and_never_enrolls_queries_vtd_or_writes_business() {
    let f=Fixture::new(reuse_steps(true,false),true);configure_reuse(&f,false);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.phase,"completed");assert_eq!(v.intent,LoginIntent::ReuseHosting);assert_eq!(v.business_state,"already_enabled");assert!(v.line_id.is_some());
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip","/state"]);
    let seen=lock(&f.script.seen);assert!(seen[0].body.get("enrollId").is_none());
    for at in [2,5] {assert_eq!(seen[at].body,json!({"device_id":PHONE_UUID}));assert_eq!(seen[at].wire,seen[at].body);}
    for record in seen.iter().skip(1) {assert_eq!(record.query["token"],"fixture-app-session");}
    drop(seen);let observation=v.device_trust.as_ref().unwrap();
    assert_eq!(observation.call_status.as_ref().unwrap().authentication_code,Some(1));
    assert_eq!(observation.security_verify.state,device_trust::TrustQueryState::NotRequested);
    assert_eq!(observation.existing_device.state,device_trust::TrustQueryState::NotRequested);
    assert_eq!(observation.local_cs_forward,Some(2));assert_eq!(observation.device_sync.as_ref().unwrap().state,DeviceSyncState::NotConfigured);
    assert_eq!(f.manager.device_identities().unwrap().identities[0].device_id,PHONE_UUID);assert_eq!(f.factory.build_count(),0);
    assert_redacted(&v);f.manager.shutdown().await;
}
#[tokio::test]
async fn unknown_untrusted_and_noncanonical_reuse_answers_stop_before_phone_credentials_even_when_business_is_on() {
    for auth in [json!(0),json!(2),json!("01"),json!(true),json!(null),json!(999999)] {
        let mut s=owned_login_steps();s.push(state(true,auth));let f=Fixture::new(s,false);configure_reuse(&f,true);
        let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
        assert_eq!(v.error,Some("app_login_device_untrusted"));assert_eq!(v.business_state,"not_requested");
        assert_eq!(f.script.paths(),["/login","/user","/state"]);assert_no_native(&f,&v).await;f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn reuse_transport_or_rejected_status_is_not_a_grant_and_never_falls_back_to_new_vtd() {
    for last in [Step::Fail("/state".into()),reply("/state",json!({"code":506,"body":{"authentication_code":1,"token":"reflected-private-token"}}))] {
        let mut s=owned_login_steps();s.push(last);let f=Fixture::new(s,false);configure_reuse(&f,false);
        let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
        assert!(v.error.is_some());assert_eq!(v.device_trust.as_ref().unwrap().call_status.as_ref().unwrap().state,device_trust::TrustQueryState::Failed);
        assert_eq!(f.script.paths(),["/login","/user","/state"]);assert_no_native(&f,&v).await;
        assert!(!serde_json::to_string(&v).unwrap().contains("reflected-private-token"));f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn reuse_of_an_off_service_does_not_sync_fetch_phone_material_or_enable_without_separate_business_intent() {
    let mut s=owned_login_steps();s.push(state(false,json!(1)));let f=Fixture::new(s,false);configure_reuse(&f,true);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_business_not_enabled"));assert_eq!(v.business_state,"not_requested");
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::NotRequested);
    assert_eq!(f.script.paths(),["/login","/user","/state"]);assert_no_native(&f,&v).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn trust_changed_while_credentials_were_fetched_stops_before_account_creation_or_business_write() {
    for intent in [LoginIntent::ReuseHosting,LoginIntent::EnableTrustedHosting] {
        let mut s=reuse_steps(true,false);*s.last_mut().unwrap()=state(true,json!(2));let f=Fixture::new(s,false);configure_reuse(&f,false);
        let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,intent)).unwrap();let v=terminal(&f.manager,&attempt).await;
        assert_eq!(v.error,Some("app_login_device_untrusted"));assert_eq!(v.business_state,"not_requested");
        assert_eq!(v.device_trust.as_ref().unwrap().call_status.as_ref().unwrap().authentication_code,Some(2));assert_no_native(&f,&v).await;f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn explicitly_enabling_with_the_trusted_original_uuid_confirms_auth_and_business_without_a_vtd_write() {
    let mut s=reuse_steps(false,false);s.push(reply("/enable",json!({"code":0})));s.push(state(true,json!(1)));
    let f=Fixture::new(s,true);configure_reuse(&f,false);let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::EnableTrustedHosting)).unwrap();
    let v=terminal(&f.manager,&attempt).await;assert_eq!(v.phase,"completed");assert_eq!(v.business_state,"enabled");assert!(v.line_id.is_some());assert_redacted(&v);
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip","/state","/enable","/state"]);
    assert_eq!(lock(&f.script.seen)[6].body,json!({"cs_forward":"2","device_id":PHONE_UUID}));assert_eq!(f.factory.build_count(),0);f.manager.shutdown().await;
}
#[tokio::test]
async fn uncertain_trusted_business_enable_is_durable_no_account_and_same_attempt_is_not_replayed() {
    let mut s=reuse_steps(false,false);s.push(Step::Fail("/enable".into()));let f=Fixture::new(s,false);configure_reuse(&f,false);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::EnableTrustedHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_transport_failed"));assert_eq!(v.business_state,"request_uncertain");assert_no_native(&f,&v).await;
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::EnableTrustedHosting)).unwrap_err().code,"app_login_attempt_exists");
    assert_eq!(f.script.paths().iter().filter(|s|*s=="/enable").count(),1);f.manager.shutdown().await;
}
#[tokio::test]
async fn accepted_trusted_business_requires_both_current_auth_and_enabled_readback_before_a_receiver_exists() {
    for final_state in [state(false,json!(1)),state(true,json!(2))] {
        let mut s=reuse_steps(false,false);s.push(reply("/enable",json!({"code":0})));s.push(final_state);let f=Fixture::new(s,false);configure_reuse(&f,false);
        let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::EnableTrustedHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
        assert!(matches!(v.error,Some("app_login_business_unconfirmed")|Some("app_login_device_untrusted")));
        assert_eq!(v.business_state,"accepted_unconfirmed");assert_no_native(&f,&v).await;f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn rejected_trusted_enable_506_is_not_new_enrollment_or_a_receiver_start() {
    let mut s=reuse_steps(false,false);s.push(reply("/enable",json!({"code":506,"body":{"authentication_code":1}})));
    let f=Fixture::new(s,false);configure_reuse(&f,false);let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::EnableTrustedHosting)).unwrap();
    let v=terminal(&f.manager,&attempt).await;assert_eq!(v.error,Some("app_login_device_untrusted"));assert_eq!(v.server_code,Some(506));assert_eq!(v.business_state,"rejected");
    assert_no_native(&f,&v).await;assert!(!f.script.paths().iter().any(|s|matches!(s.as_str(),"/enroll"|"/trust")));f.manager.shutdown().await;
}
#[test]
fn reuse_of_a_known_phone_uuid_adds_an_alias_only_after_matching_the_same_confirmed_number() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",100).unwrap();
    let first=device::resolve_reuse(&conn,&[1;32],PHONE_UUID,100).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM jegotrip_device_aliases",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    let first=device::confirm_reuse(&conn,&[1;32],&first,&[3;32],101).unwrap();
    let other=device::resolve_reuse(&conn,&[2;32],PHONE_UUID,102).unwrap();assert_eq!(other.id,first.id);
    assert_eq!(device::confirm_reuse(&conn,&[2;32],&other,&[4;32],103).unwrap_err().code,"app_login_identity_mismatch");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM jegotrip_device_aliases",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    let same=device::confirm_reuse(&conn,&[2;32],&other,&[3;32],103).unwrap();assert_eq!(same.id,first.id);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM jegotrip_device_identities",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM jegotrip_device_aliases",[],|r|r.get::<_,i64>(0)).unwrap(),2);
}
#[test]
fn another_canonical_uuid_is_not_silently_selected_and_the_original_identity_is_not_retired() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",100).unwrap();
    let original=device::resolve_reuse(&conn,&[1;32],PHONE_UUID,100).unwrap();let original=device::confirm_reuse(&conn,&[1;32],&original,&[3;32],101).unwrap();
    let selected=device::resolve_reuse(&conn,&[2;32],OTHER_UUID,102).unwrap();
    assert_eq!(device::confirm_reuse(&conn,&[2;32],&selected,&[3;32],103).unwrap_err().code,"app_login_device_identity_conflict");
    assert_eq!(device::list(&conn).unwrap().len(),2);
    assert_eq!(conn.query_row("SELECT retired,revision FROM jegotrip_device_identities WHERE id=?1",[&original.id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap(),(0,original.revision as i64));
    assert_eq!(conn.query_row("SELECT retired FROM jegotrip_device_identities WHERE id=?1",[&selected.id],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM jegotrip_device_aliases",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}
#[test]
fn reuse_cannot_resurrect_retired_or_rotated_phone_uuid_history() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",100).unwrap();
    let original=device::resolve_reuse(&conn,&[1;32],PHONE_UUID,100).unwrap();
    device::set(&conn,Some(&original.id),original.revision,None,Some(OTHER_UUID),101).unwrap();
    assert_eq!(device::resolve_reuse(&conn,&[2;32],PHONE_UUID,102).unwrap_err().code,"app_login_device_identity_conflict");
    let active=device::list(&conn).unwrap().remove(0);device::remove(&conn,&active.id,active.revision,103).unwrap();
    assert_eq!(device::resolve_reuse(&conn,&[2;32],OTHER_UUID,104).unwrap_err().code,"app_login_device_identity_conflict");
}
#[tokio::test]
async fn reused_uuid_lease_and_failed_admission_do_not_allocate_or_retire_more_device_values() {
    let f=Fixture::new(vec![],false);configure_reuse(&f,false);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER refuse_reuse_admission BEFORE INSERT ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&id(),LoginIntent::ReuseHosting)).unwrap_err().code,"app_login_storage_failed");
    for table in ["jegotrip_device_identities","jegotrip_device_aliases","jegotrip_device_identity_history"] {assert_eq!(rows(&f,table),0);}
    assert!(f.script.paths().is_empty());
    f.temp.database.with_conn(|c|c.execute_batch("DROP TRIGGER refuse_reuse_admission;").map_err(|_|"fixture")).unwrap();
    lock(&f.script.steps).push_back(Step::Park("/login".into()));let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async {while f.script.paths().is_empty(){tokio::task::yield_now().await;}}).await.unwrap();
    let mut another=reuse_request(&id(),LoginIntent::ReuseHosting);another.username=Secret::from_owned("another-alias@example.invalid".into());
    assert_eq!(f.manager.submit_jegotrip(another).unwrap_err().code,"app_login_busy");assert_eq!(rows(&f,"jegotrip_device_identities"),1);
    assert_eq!(rows(&f,"jegotrip_device_identity_history"),1);f.manager.shutdown().await;assert_no_native(&f,&f.manager.get(&attempt).unwrap()).await;
}
#[test]
fn known_reused_phone_uuid_with_unjoined_receiver_is_refused_before_app_signin() {
    let f=Fixture::new(vec![],false);configure_reuse(&f,false);
    f.temp.database.with_conn(|c| {
        let summary=accounts::record(c,&[7;32],"u***","***0123",1,100).map_err(|e|e.code)?;
        device::set(c,None,0,Some(&[7;32]),Some(PHONE_UUID),100).map_err(|e|e.code)?;
        let line="app-account-0123456789abcdef0123456789abcdef";
        accounts::link_voice_line(c,&summary,line)?;account_runs::initialize_schema(c)?;
        c.execute("INSERT INTO app_voice_runtime_leases(line_id,revision,run_nonce) VALUES(?1,1,?2)",rusqlite::params![line,"0123456789abcdef0123456789abcdef"]).map_err(|_|"fixture")?;Ok(())
    }).unwrap();
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&id(),LoginIntent::ReuseHosting)).unwrap_err().code,"app_login_device_identity_in_use");
    assert_eq!(rows(&f,"jegotrip_login_attempts"),0);assert!(f.script.paths().is_empty());assert_eq!(f.factory.build_count(),0);
}
#[tokio::test]
async fn old_successful_trust_observation_cannot_authorize_a_later_reuse_when_the_server_now_returns_two() {
    let mut s=trust_check_steps(1,true);let mut next=owned_login_steps();next.push(state(true,json!(2)));s.extend(next);
    let f=Fixture::new(s,false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:reuse_configuration(&f,false)}).unwrap();
    let first=id();f.manager.submit_jegotrip(trust_check_request(&first)).unwrap();let previous=terminal(&f.manager,&first).await;idle(&f).await;
    assert_eq!(previous.device_trust.as_ref().unwrap().existing_device.authentication_code,Some(1));
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_device_untrusted"));assert_eq!(v.account_id,previous.account_id);assert_no_native(&f,&v).await;
    assert_eq!(f.manager.get(&first).unwrap().device_trust.unwrap().existing_device.authentication_code,Some(1));f.manager.shutdown().await;
}
#[test]
fn observed_phone_report_profile_is_bounded_private_and_cannot_override_uuid_account_or_grant_fields() {
    let profile=sync_profile();profile.validate().unwrap();let payload=profile.payload(PHONE_UUID,"fixture-current-accountid").unwrap();
    assert_eq!(payload.0["device_id"],PHONE_UUID);assert_eq!(payload.0["imei"],PHONE_UUID);assert_eq!(payload.0["userId"],"fixture-current-accountid");
    assert_eq!(payload.0["os_type"],"0");assert_eq!(payload.0["app_version_code"],8980);
    assert!(payload.0.get("device_token").is_none());assert!(payload.0.get("macAddress").is_none());
    assert!(!format!("{profile:?}").contains("fixture-phone"));
    let value=serde_json::to_value(profile.private_mirror()).unwrap();
    for field in ["device_id","imei","userId","accountid","sso_token","trusted"] {
        let mut bad=value.clone();bad[field]=json!("must-not-import");assert!(serde_json::from_value::<device_sync::DeviceSyncProfile>(bad).is_err());
    }
    for (field,bad) in [("app_version",json!("8.9..8")),("app_version_code",json!(0)),("app_version_code",json!(2147483648u64)),
        ("device_type",json!("x".repeat(129))),("os_version",json!("bad\nversion")),("device_token",json!("")),("device_token",json!("x".repeat(4097)))] {
        let mut bad_value=value.clone();bad_value[field]=bad;
        let bad:device_sync::DeviceSyncProfile=serde_json::from_value(bad_value).unwrap();assert!(bad.validate().is_err(),"{field}");
    }
}
#[test]
fn device_sync_profile_roundtrip_preserves_independent_sdk_context_and_requires_a_valid_endpoint() {
    let f=Fixture::new(vec![],false);let c=reuse_configuration(&f,true);let original_sdk=c.runtime.as_ref().unwrap().sdk.device_id.as_ref().map(|s|s.expose_to_backend().to_owned());
    let encoded=c.encode_private().unwrap();let decoded=DatabaseConfiguration::decode_private(&encoded).unwrap();
    assert_eq!(decoded.http.device_sync_profile.as_ref().unwrap().app_version_code,8980);
    assert_eq!(decoded.runtime.as_ref().unwrap().sdk.device_id.as_ref().map(|s|s.expose_to_backend().to_owned()),original_sdk);
    assert_ne!(original_sdk.as_deref(),Some(PHONE_UUID));
    let mut missing=c.clone();missing.http.endpoints.device_sync=None;assert!(missing.validate().is_err());
    let mut bad=c;bad.http.endpoints.device_sync=Some("https://another.invalid/write".into());assert!(bad.validate().is_err());
    let public=serde_json::to_string(&f.manager.configuration().unwrap()).unwrap();assert!(!public.contains("fixture-phone"));
}
#[tokio::test]
async fn configured_original_device_sync_uses_current_accountid_and_uuid_and_omits_unknown_push_material() {
    let f=Fixture::new(reuse_steps(true,true),true);configure_reuse(&f,true);let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();
    let v=terminal(&f.manager,&attempt).await;assert_eq!(v.phase,"completed");assert_redacted(&v);
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip","/state","/device-sync","/state"]);
    let seen=lock(&f.script.seen);let payload=&seen[6].body;
    assert_eq!(payload["device_id"],PHONE_UUID);assert_eq!(payload["imei"],PHONE_UUID);assert_eq!(payload["userId"],"fixture-owned-app-account-id");
    assert_ne!(payload["userId"],"must-not-substitute-user-id");assert!(payload.get("device_token").is_none());assert!(payload.get("macAddress").is_none());
    assert_eq!(seen[6].query["token"],"fixture-app-session");assert_ne!(seen[6].wire,seen[6].body,"device report follows original encrypted branch");drop(seen);
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::Accepted);
    let public=serde_json::to_string(&v).unwrap();assert!(!public.contains("must-not-retain-sync-token"));assert!(!public.contains("fixture-phone"));f.manager.shutdown().await;
}
#[tokio::test]
async fn configured_sync_without_current_accountid_is_not_guessed_from_user_id_number_or_profile() {
    let mut s=app_only_steps();s.push(state(true,json!(1)));let f=Fixture::new(s,false);configure_reuse(&f,true);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_device_sync_context_missing"));assert_eq!(f.script.paths(),["/login","/user","/state"]);
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::NotRequested);assert_no_native(&f,&v).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn uncertain_device_sync_is_durable_before_send_and_is_not_repeated_or_used_as_trust() {
    let mut s=reuse_steps(true,false);s.push(Step::Fail("/device-sync".into()));let f=Fixture::new(s,false);configure_reuse(&f,true);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_transport_failed"));assert_eq!(v.business_state,"not_requested");assert_no_native(&f,&v).await;
    let sync=v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap();assert_eq!(sync.state,DeviceSyncState::RequestUncertain);
    assert_eq!(sync.error.as_deref(),Some("app_login_transport_failed"));
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap_err().code,"app_login_attempt_exists");
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip","/state","/device-sync"]);f.manager.shutdown().await;
}
#[tokio::test]
async fn a_parked_sync_has_an_uncertain_ledger_before_transport_and_shutdown_never_replays_it() {
    let mut s=reuse_steps(true,false);s.push(Step::Park("/device-sync".into()));let f=Fixture::new(s,false);configure_reuse(&f,true);
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async {while !f.script.paths().iter().any(|s|s=="/device-sync"){tokio::task::yield_now().await;}}).await.unwrap();
    let v=f.manager.get(&attempt).unwrap();assert_eq!(v.phase,"syncing_device");assert!(!v.completed);
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::RequestUncertain);
    f.manager.shutdown().await;let v=f.manager.get(&attempt).unwrap();assert_eq!(v.phase,"interrupted");assert_no_native(&f,&v).await;
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::RequestUncertain);
}
#[tokio::test]
async fn profile_and_extra_fields_cannot_sneak_an_enrollment_or_foreign_report_owner_into_reuse() {
    let f=Fixture::new(vec![],false);let mut c=reuse_configuration(&f,false);
    c.http.extra_body=vec![config::HttpPair {name:Secret::from_owned("enrollId".into()),value:Secret::from_owned("fixture-old".into())}];
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:c}).unwrap();
    assert_eq!(f.manager.submit_jegotrip(reuse_request(&id(),LoginIntent::ReuseHosting)).unwrap_err().code,"app_login_deployment_invalid");
    assert!(f.script.paths().is_empty());assert_eq!(rows(&f,"jegotrip_device_identities"),0);
    let cfg=reuse_configuration(&f,false);let mut client=Factory(f.script.clone()).build(&cfg.http).unwrap().with_device_id(PHONE_UUID).with_trusted_reuse().unwrap();
    client.ensure_device_enrollment().await;assert_eq!(client.enroll_device().await.unwrap_err().code,"app_login_request_invalid");
    assert_eq!(client.verify_trusted_device().await.unwrap_err().code,"app_login_request_invalid");assert!(f.script.paths().is_empty());
    client.reset_session();assert!(f.script.paths().is_empty());
}
#[test]
fn reuse_observations_keep_the_old_two_kib_contract_and_never_serialize_report_or_account_fields() {
    let snapshot=device::Snapshot {id:id(),device_id:Secret::from_owned(PHONE_UUID.into()),revision:1};
    let status=http::CallStatus {authentication_code:Some(1),hosting_enabled:true,local_cs_forward:Some(2)};
    let observation=DeviceTrustObservation::capture_reuse(&snapshot,100,Ok(status),DeviceSyncObservation::request_uncertain());
    let encoded=observation.encode().unwrap();assert!(encoded.len()<=2048);assert!(!encoded.contains(PHONE_UUID));assert!(!encoded.contains("accountid"));
    let decoded=DeviceTrustObservation::decode(&encoded).unwrap();assert_eq!(decoded.call_status.unwrap().authentication_code,Some(1));
    let old=DeviceTrustObservation::capture(&snapshot,100,Ok(None),Ok(None),Ok(true)).encode().unwrap();
    let decoded=DeviceTrustObservation::decode(&old).unwrap();assert!(decoded.call_status.is_none());assert!(decoded.device_sync.is_none());
    let mut reflected:Value=serde_json::from_str(&encoded).unwrap();reflected["device_token"]=json!("secret");assert!(DeviceTrustObservation::decode(&reflected.to_string()).is_err());
}
#[tokio::test]
async fn reuse_observation_write_failure_prevents_even_the_optional_device_report_or_phone_request() {
    let mut s=owned_login_steps();s.push(state(true,json!(1)));let f=Fixture::new(s,false);configure_reuse(&f,true);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER refuse_reuse_observation BEFORE UPDATE OF device_trust ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_storage_failed"));assert_eq!(f.script.paths(),["/login","/user","/state"]);assert_no_native(&f,&v).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn trusted_reuse_status_uses_real_pinned_https_and_keeps_the_original_plain_json_shape() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server=cert.server_config(&rustls::version::TLS13);
    let serving=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move {
        let mut requests=vec![];
        for body in [r#"{"code":0,"body":{"token":"fixture-owned-https-session"}}"#,r#"{"code":0,"body":{"cs_forward":"2","authentication_code":"1","localCsForward":"2"}}"#] {
            let (tcp,_)=listener.accept().await.unwrap();let mut stream=tokio_rustls::TlsAcceptor::from(server.clone()).accept(tcp).await.unwrap();
            let mut request=vec![];let mut buf=[0u8;2048];
            loop {let n=stream.read(&mut buf).await.unwrap();assert!(n>0);request.extend_from_slice(&buf[..n]);assert!(request.len()<65536);
                if let Some(end)=request.windows(4).position(|w|w==b"\r\n\r\n") {
                    let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|s|s.trim().parse::<usize>().unwrap()).unwrap_or(0);
                    if request.len()>=end+4+length {break;}
                }
            }
            requests.push(request);let response=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            stream.write_all(response.as_bytes()).await.unwrap();let _=stream.shutdown().await;
        }requests
    }).await.unwrap()});
    let mut c=http_config(false);c.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));c.root_der_base64=None;
    c.pinned_server_certificate_der_base64=Some(Secret::from_owned(cert.root.clone()));c.pinned_server_public_key_der_base64=Some(Secret::from_owned(cert.public_key.clone()));
    c.enroll_id=Some(Secret::from_owned("fixture-stale-enrolment".into()));
    let mut client=LiveFactory.build(&c).unwrap().with_device_id(PHONE_UUID).with_trusted_reuse().unwrap();client.login(&login_request(&id())).await.unwrap();
    let status=client.call_status().await.unwrap();assert_eq!(status.authentication_code,Some(1));assert!(status.hosting_enabled);
    let requests=serving.await.unwrap();let first=String::from_utf8(requests[0].clone()).unwrap();assert!(!first.contains("enrollId"));assert!(!first.contains("p@ss"));
    let last=String::from_utf8(requests[1].clone()).unwrap();assert!(last.starts_with("POST /state?lang=zh_CN&token=fixture-owned-https-session"));
    let body=last.split("\r\n\r\n").nth(1).unwrap();assert_eq!(serde_json::from_str::<Value>(body).unwrap(),json!({"device_id":PHONE_UUID}));
}
#[tokio::test]
async fn trusted_reuse_post_sync_enable_native_sms_ack_and_stop_logout_join_keep_the_original_scope_and_tokens() {
    let mut s=reuse_steps(false,true);s.push(reply("/enable",json!({"code":0})));s.push(state(true,json!(1)));
    let f=Fixture::new(s,false);configure_reuse(&f,true);let (mut native,_,dropped)=peer(&f.cert,&f.factory);
    let serving=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(12),async move {
        native.authenticate_as(b"[username:+447700900123@fixture.invalid]",b"fixture-original-pwd").await;
        native.swap(Some(4),true).await;native.incoming_with_fields(93,b"Sip.Request",b"fixture-reused-phone-SMS",Some(b"fixture-sender"),None).await;
        assert_eq!(next_non_poll(&mut native).await.kind,transport_packets::CallKind::Reply);
        native.swap(Some(3),true).await;let message=next_non_poll(&mut native).await;
        let request=decode_request(message.payload.expose_to_transport()).unwrap();assert_eq!(request.method,rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        native.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(native.receive().await.is_none());
    }).await.unwrap()});
    let attempt=id();let mut request=reuse_request(&attempt,LoginIntent::EnableTrustedHosting);request.start_receiver=true;f.manager.submit_jegotrip(request).unwrap();
    let v=terminal(&f.manager,&attempt).await;assert_eq!(v.phase,"completed");assert_eq!(v.business_state,"enabled");assert_redacted(&v);let line=v.line_id.unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {loop {
        let state=f.accounts.get(&line,now() as u64).await.unwrap();
        if state.incoming_message_ready && f.accounts.messages(&line,10).unwrap().len()==1 {assert!(!state.incoming_call_ready);assert!(!state.outbound_authorized);break;}
        tokio::task::yield_now().await;
    }}).await.unwrap();
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip","/state","/device-sync","/state","/enable","/state"]);
    f.accounts.stop(&line,1,now() as u64).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert_eq!(f.factory.build_count(),1);
    f.manager.shutdown().await;
}

#[tokio::test]
async fn invalid_native_context_stops_before_even_an_opted_in_device_sync_write() {
    let mut s=owned_login_steps();s.push(state(true,json!(1)));s.extend(prefix().into_iter().skip(2));
    let f=Fixture::new(s,false);let mut c=reuse_configuration(&f,true);
    c.runtime.as_mut().unwrap().sdk.application_key=Secret::from_owned(String::new());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:c}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_native_configuration_invalid"));assert_eq!(v.business_state,"not_requested");
    assert_eq!(f.script.paths(),["/login","/user","/state","/csb/token","/voip"]);
    assert_eq!(v.device_trust.as_ref().unwrap().device_sync.as_ref().unwrap().state,DeviceSyncState::NotRequested);
    assert_no_native(&f,&v).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn trust_changed_during_credentials_prevents_device_sync_and_accepted_sync_is_not_a_later_trust_grant() {
    for after_report in [false,true] {
        let mut s=reuse_steps(true,after_report);*s.last_mut().unwrap()=state(true,json!(2));
        let f=Fixture::new(s,false);configure_reuse(&f,true);let attempt=id();
        f.manager.submit_jegotrip(reuse_request(&attempt,LoginIntent::ReuseHosting)).unwrap();let v=terminal(&f.manager,&attempt).await;
        assert_eq!(v.error,Some("app_login_device_untrusted"));assert_eq!(v.business_state,"not_requested");
        let obs=v.device_trust.as_ref().unwrap();assert_eq!(obs.call_status.as_ref().unwrap().authentication_code,Some(2));
        assert_eq!(obs.device_sync.as_ref().unwrap().state,if after_report {DeviceSyncState::Accepted}else{DeviceSyncState::NotRequested});
        assert_eq!(f.script.paths().iter().filter(|s|*s=="/device-sync").count(),if after_report {1}else{0});
        assert_no_native(&f,&v).await;f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn trusted_reuse_write_marker_survives_session_reset_for_sync_and_explicit_business_attempts() {
    for sync in [false,true] {
        let mut s=owned_login_steps();s.push(state(true,json!(1)));
        if sync {s.push(reply("/device-sync",json!({"code":0})));} else {s.push(reply("/enable",json!({"code":524})));}
        let f=Fixture::new(s,false);let c=reuse_configuration(&f,sync);
        let mut client=Factory(f.script.clone()).build(&c.http).unwrap().with_device_id(PHONE_UUID).with_trusted_reuse().unwrap();
        let request=login_request(&id());client.login(&request).await.unwrap();let bound=client.bound_number(&request).await.unwrap();
        let status=client.call_status().await.unwrap();assert_eq!(status.authentication_code,Some(1));assert!(!client.trusted_reuse_write_started());
        if sync {client.sync_device(&bound,status).await.unwrap();} else {assert_eq!(client.enable_business().await.unwrap_err().server_code,Some(524));}
        assert!(client.trusted_reuse_write_started());client.reset_session();assert!(client.trusted_reuse_write_started());
        assert_eq!(f.script.paths().len(),4);f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn a_new_reuse_alias_returning_another_canonical_uuid_stops_before_state_without_retiring_saved_uuid() {
    let mut s=trust_check_steps(1,true);s.extend(owned_login_steps());let f=Fixture::new(s,false);configure_reuse(&f,false);
    let first=id();f.manager.submit_jegotrip(trust_check_request(&first)).unwrap();let previous=terminal(&f.manager,&first).await;idle(&f).await;
    let old=f.manager.device_identities().unwrap().identities.remove(0);
    let attempt=id();let mut request=reuse_request(&attempt,LoginIntent::ReuseHosting);
    request.username=Secret::from_owned("new-alias@example.invalid".into());request.device_id=Some(Secret::from_owned(OTHER_UUID.into()));
    f.manager.submit_jegotrip(request).unwrap();let v=terminal(&f.manager,&attempt).await;
    assert_eq!(v.error,Some("app_login_device_identity_conflict"));assert!(v.account_id.is_none());assert_no_native(&f,&v).await;
    let identities=f.manager.device_identities().unwrap().identities;let retained=identities.iter().find(|i|i.id==old.id).unwrap();
    assert_eq!(retained.device_id,PHONE_UUID);assert_eq!(retained.revision,old.revision);assert_eq!(retained.bound_account_id,previous.account_id);
    assert_eq!(f.script.paths().iter().filter(|s|*s=="/state").count(),1,"only the previous diagnostic queried state");
    f.manager.shutdown().await;
}
