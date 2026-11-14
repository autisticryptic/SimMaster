//! Actions-only original App communication operations; synthetic HTTPS/body peers.
use super::*;
use super::super::communication::{CommunicationOperation as Op,CommunicationAction as Action,CommunicationObservation,CommunicationState as State,CommunicationStatus};
use base64::Engine as _;
const UUID:&str="D5C0199B-1644-4D33-8912-B8BD5AB49224";
#[path="communication_https_tests.rs"]
mod https;
struct CommScript{steps:Mutex<VecDeque<Step>>,seen:Mutex<Vec<Recorded>>,protection:Option<config::BodyProtection>}
impl CommScript{
    fn lookup(&self,text:&str)->Option<Vec<u8>>{
        let url=reqwest::Url::parse(text).unwrap();assert_eq!(url.scheme(),"https");assert_eq!(url.host_str(),Some("itunes.apple.com"));assert_eq!(url.path(),"/lookup");
        let query:BTreeMap<String,String>=url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();assert!(!query.contains_key("token")&&!query.contains_key("n_token"));
        lock(&self.seen).push(Recorded{path:url.path().to_owned(),query,body:Value::Null,wire:Value::Null});
        let step=lock(&self.steps).pop_front().expect("unexpected public-version lookup");
        match step{Step::Reply(path,200,body)=>{assert_eq!(path,url.path());Some(serde_json::to_vec(&body).unwrap())},_=>panic!("unexpected public-version fixture")}
    }
    fn paths(&self)->Vec<String>{lock(&self.seen).iter().map(|r|r.path.clone()).collect()}
}
impl Transport for CommScript{
    fn exchange(&self,request:WireRequest)->Exchange<'_>{Box::pin(async move{
        let path=request.url.path().to_owned();
        let encrypted=self.protection.is_some()&&matches!(path.as_str(),"/login"|"/user"|"/close"|"/jego"|"/display");
        let wire:Value=if request.body.is_empty(){Value::Null}else{serde_json::from_slice(&request.body).unwrap()};
        let payload=if encrypted&&path=="/display"{
            let sec=base64::engine::general_purpose::STANDARD.decode(wire["sec"].as_str().unwrap()).unwrap();let sec=String::from_utf8(sec).unwrap();
            let nonce=sec.split(';').nth(1).unwrap();
            let expected=crypto::request_body(b"",self.protection.as_ref(),nonce).unwrap();assert_eq!(wire,serde_json::from_slice::<Value>(&expected).unwrap());Value::Null
        }else if encrypted{
            let mut doc=crypto::parse(&request.body).unwrap();crypto::decrypt_body(&mut doc,self.protection.as_ref()).unwrap();doc.0["body"].clone()
        }else{wire.clone()};
        if !encrypted{assert!(wire.get("sec").is_none());}
        lock(&self.seen).push(Recorded{path:path.clone(),query:request.url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect(),body:payload,wire});
        let step=lock(&self.steps).pop_front().expect("unexpected communication request");
        match step{
            Step::Fail(expected)=>{assert_eq!(path,expected);Err(Error::new("app_login_transport_failed"))},
            Step::Park(expected)=>{assert_eq!(path,expected);std::future::pending().await},
            Step::Reply(expected,status,mut body)=>{
                assert_eq!(path,expected);
                if encrypted&&body["body"].is_object(){
                    let bytes=serde_json::to_vec(&body["body"]).unwrap();let value=crypto::request_body(&bytes,self.protection.as_ref(),"1700000000123105").unwrap();
                    let envelope:Value=serde_json::from_slice(&value).unwrap();body["body"]=envelope["body"].clone();body["sec"]=envelope["sec"].clone();
                }
                Ok(WireResponse{status,body:Zeroizing::new(serde_json::to_vec(&body).unwrap())})
            }
        }
    })}
}
struct CommFactory(Arc<CommScript>);
impl TransportFactory for CommFactory{fn build(&self,c:&HttpDeployment)->Result<AppClient,Error>{
    let mut client=AppClient::new(c.clone(),self.0.clone())?;let script=self.0.clone();client.version_lookup=Some(Arc::new(move|url|script.lookup(url)));Ok(client)
}}
fn configuration(f:&Fixture,writes:bool)->DatabaseConfiguration{
    let mut value=database_configuration(f,false);value.http.communication=Some(serde_json::from_value(json!({"allow_business_writes":writes,
        "profile":"profile","phone_service":"phone-service","display_status":"display","set_voip_profile":"set-voip","close_forward":"close","jego_m_status":"jego","logout":"logout"})).unwrap());
    // Intent must suppress this stale optional enrolment exactly as original reuse.
    value.http.endpoints.device_enroll=Some("enroll".into());value.http.enroll_id=Some(Secret::from_owned("fixture-stale-enrolment".into()));value
}
fn fixture(steps:Vec<Step>,encrypted:bool,writes:bool)->(Fixture,Arc<CommScript>){
    let mut f=Fixture::new(vec![],encrypted);let script=Arc::new(CommScript{steps:Mutex::new(steps.into()),seen:Mutex::new(Vec::new()),protection:http_config(encrypted).body_protection});
    Arc::get_mut(&mut f.manager).unwrap().factory=Arc::new(CommFactory(script.clone()));
    f.manager.configure(ConfigurationWriteRequest{expected_revision:0,configuration:configuration(&f,writes)}).unwrap();(f,script)
}
fn request(id:&str,operation:Value)->JegotripLoginRequest{serde_json::from_value(json!({"attempt_id":id,"username":"user@example.invalid","password":" p@ss ","country_code":null,
    "intent":"communication","consent":true,"write_consent":true,"start_receiver":false,"device_id":UUID,"expected_configuration_revision":1,"communication_operation":operation})).unwrap()}
fn complete_reply(path:&str,body:Value)->Step{reply(path,json!({"code":0,"body":body}))}
fn profile_body()->Value{json!({"list":[{"allow_mo_voice":1,"allow_mt_voice":0,"mo_voice_limit":-1,"mt_voice_limit":900,
    "forbidden_country":[{"country_code":"GB","country_cname":"must-not-reflect-token","region_id":9223372036854775807i64}],"answer_forbidden_country":[{"country_code":"44","region_id":7}],"token":"must-not-reflect"}]})}
async fn idle(f:&Fixture){tokio::time::timeout(Duration::from_secs(4),async{while f.manager.active.available_permits()!=2{tokio::task::yield_now().await;}}).await.unwrap();}
async fn no_native(f:&Fixture,view:&AttemptView){assert_eq!(view.intent,LoginIntent::Communication);assert!(view.line_id.is_none());assert_eq!(view.business_state,"not_requested");assert!(view.device_trust.is_none());assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());assert_eq!(f.factory.build_count(),0);assert_redacted(view);}

#[test]
fn communication_operations_have_no_url_token_grant_or_implicit_receiver_fields(){
    for kind in ["query_profile","query_phone_service","query_display_status","query_forwarding_state","close_forward","logout"]{
        assert!(serde_json::from_value::<Op>(json!({"kind":kind})).is_ok());
        for key in ["url","token","device_id","trust_granted","start_receiver","phone_login_token","sip_password"]{let mut value=json!({"kind":kind});value[key]=json!("fixture-private");assert!(serde_json::from_value::<Op>(value).is_err());}
    }
    for value in [json!({"kind":"set_phone_service","status":-1}),json!({"kind":"set_phone_service","status":1000000}),json!({"kind":"set_voip_profile","cs_forward":"02"}),json!({"kind":"set_jego_m_status","jegom_status":"7","region_code":"44","country_code":"44"})]{assert!(serde_json::from_value::<Op>(value).unwrap().validate().is_err());}
    assert!(serde_json::from_value::<Op>(json!({"kind":"query_vtd"})).is_err());
}
#[test]
fn communication_configuration_is_optional_default_denied_and_byte_stable_when_absent(){
    let f=Fixture::new(Vec::new(),false);let old=database_configuration(&f,false);let bytes=old.encode_private().unwrap();let value:Value=serde_json::from_slice(&bytes).unwrap();assert!(value["http"].get("communication").is_none());
    let old=DatabaseConfiguration::decode_private(&bytes).unwrap();assert!(old.http.communication.is_none());assert_eq!(&*bytes,&*old.encode_private().unwrap());
    let config=configuration(&f,false);let api=config.http.communication.as_ref().unwrap();assert_eq!(api.available(),[Action::QueryProfile,Action::QueryPhoneService,Action::QueryDisplayStatus,Action::QueryForwardingState]);
    assert_eq!(api.preflight(&Op::Logout{}).unwrap_err().code,"app_login_communication_writes_disabled");
}
#[test]
fn communication_configuration_rejects_foreign_plain_and_ambiguous_origins(){
    let f=Fixture::new(Vec::new(),false);
    for path in ["http://fixture.invalid/profile","https://other.invalid/profile","//other.invalid/profile","profile?token=fixture","../profile","profile#fragment"]{
        let mut c=configuration(&f,true);c.http.communication.as_mut().unwrap().profile=Some(path.into());assert!(c.validate().is_err(),"{path}");
    }
    let c=configuration(&f,true);assert!(c.validate().is_ok());assert_eq!(DatabaseConfiguration::decode_private(&c.encode_private().unwrap()).unwrap().http.communication.unwrap().available().len(),9);
}
#[test]
fn communication_preflight_requires_original_uuid_revision_owner_consent_and_separate_write_consent(){
    let (f,script)=fixture(Vec::new(),false,true);
    for operation in [json!({"kind":"query_profile"}),json!({"kind":"set_phone_service","status":3}),json!({"kind":"logout"})]{
        for which in 0..4{let mut r=request(&id(),operation.clone());match which{0=>r.device_id=None,1=>r.expected_configuration_revision=None,2=>r.start_receiver=true,_=>r.consent=false};assert_eq!(f.manager.submit_jegotrip(r).unwrap_err().code,"app_login_request_invalid");}
    }
    let mut r=request(&id(),json!({"kind":"close_forward"}));r.write_consent=false;assert_eq!(f.manager.submit_jegotrip(r).unwrap_err().code,"app_login_request_invalid");
    let mut r=request(&id(),json!({"kind":"query_profile"}));r.expected_configuration_revision=Some(0);assert_eq!(f.manager.submit_jegotrip(r).unwrap_err().code,"app_login_configuration_revision_conflict");
    assert!(script.paths().is_empty());assert!(f.manager.list().unwrap().is_empty());assert!(f.manager.device_identities().unwrap().identities.is_empty());
}
#[test]
fn communication_policy_refuses_unconfigured_or_disabled_operations_before_identity_or_network(){
    let (f,script)=fixture(Vec::new(),false,false);
    assert_eq!(f.manager.submit_jegotrip(request(&id(),json!({"kind":"logout"}))).unwrap_err().code,"app_login_communication_writes_disabled");
    let mut c=configuration(&f,false);c.http.communication.as_mut().unwrap().profile=None;f.manager.configure(ConfigurationWriteRequest{expected_revision:1,configuration:c}).unwrap();
    let mut r=request(&id(),json!({"kind":"query_profile"}));r.expected_configuration_revision=Some(2);assert_eq!(f.manager.submit_jegotrip(r).unwrap_err().code,"app_login_communication_not_configured");
    assert!(script.paths().is_empty());assert!(f.manager.list().unwrap().is_empty());assert!(f.manager.device_identities().unwrap().identities.is_empty());
}
#[test]
fn communication_profile_decodes_first_list_limits_countries_and_exact_i64_regions_without_grants(){
    let status=CommunicationStatus::from_body(profile_body().as_object().unwrap()).unwrap();let profile=status.profile.unwrap();assert_eq!(profile.mo_voice_limit,Some(-1));assert_eq!(profile.allow_mt_voice,Some(0));assert_eq!(profile.forbidden_countries[0].region_id.as_deref(),Some("9223372036854775807"));
    assert_eq!(profile.answer_forbidden_countries[0].country_code,"44");let text=serde_json::to_string(&profile).unwrap();assert!(!text.contains("must-not-reflect"));
    for value in [json!({"list":{}}),json!({"list":[{"allow_mo_voice":true}]}),json!({"list":[{"mo_voice_limit":"01"}]}),json!({"list":[{"forbidden_country":[{"country_code":"private-token"}]}]})]{assert!(CommunicationStatus::from_body(value.as_object().unwrap()).is_err());}
}
#[test]
fn communication_status_canonical_types_and_masking_never_reflect_credentials(){
    let status=CommunicationStatus::from_body(json!({"phoneServerStatus":"3","cs_forward":2,"jegom_status":"1","region_code":"44","jegom_phone":"+447700900123","sip_password":"private","token":"private"}).as_object().unwrap()).unwrap();
    assert_eq!(status.phone_server_status,Some(3));assert_eq!(status.masked_jegom_phone.as_deref(),Some("***0123"));let text=serde_json::to_string(&status).unwrap();assert!(!text.contains("private"));assert!(!text.contains("7700900123"));
    for value in [json!(true),json!(null),json!(-1),json!(1000000),json!(1.5),json!("01"),json!("1 ")]{assert!(CommunicationStatus::from_body(json!({"phoneServerStatus":value}).as_object().unwrap()).is_err());}
}
#[test]
fn communication_observation_storage_is_owner_phase_and_immutable_context_cas_guarded(){
    let c=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&c,"owner",1).unwrap();let task=id();store::insert_with_intent(&c,&task,"owner",LoginIntent::Communication,1).unwrap();
    let mut obs=CommunicationObservation::initial(&Op::QueryPhoneService{},1,&id(),4,2);obs.state=State::RequestUncertain;
    assert!(store::set_communication(&c,&task,"owner",&obs,2).is_err());store::step(&c,&task,"owner","communication_request",None,2).unwrap();
    assert!(store::set_communication(&c,&task,"foreign",&obs,2).is_err());store::set_communication(&c,&task,"owner",&obs,2).unwrap();
    for field in 0..3{let mut bad=obs.clone();match field{0=>bad.device_identity_id=id(),1=>bad.configuration_revision=2,_=>bad.action=Action::QueryProfile};assert!(store::set_communication(&c,&task,"owner",&bad,3).is_err());}
    obs.state=State::Observed;obs.status=Some(CommunicationStatus{phone_server_status:Some(1),..Default::default()});store::set_communication(&c,&task,"owner",&obs,3).unwrap();store::finish(&c,&task,"owner",None,4).unwrap();assert!(store::set_communication(&c,&task,"owner",&obs,5).is_err());
    let text=obs.encode().unwrap();let mut bad:Value=serde_json::from_str(&text).unwrap();bad["token"]=json!("private");assert!(CommunicationObservation::decode(&bad.to_string()).is_err());
    assert!(CommunicationObservation::decode(&"x".repeat(32769)).is_err());c.execute("UPDATE jegotrip_login_attempts SET communication=?1 WHERE attempt_id=?2",rusqlite::params![vec![1u8;32769],task]).unwrap();assert!(store::get(&c,&task).is_err());
}
#[test]
fn communication_unresolved_write_survives_restart_and_never_reinitializes_a_nonce(){
    let c=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&c,"old",1).unwrap();let task=id();let device=id();store::insert_with_intent(&c,&task,"old",LoginIntent::Communication,1).unwrap();store::step(&c,&task,"old","communication_request",None,2).unwrap();
    let mut obs=CommunicationObservation::initial(&Op::Logout{},1,&device,4,2);obs.state=State::RequestUncertain;store::set_communication(&c,&task,"old",&obs,2).unwrap();assert!(store::set_communication(&c,&task,"old",&obs,2).is_err());
    store::initialize(&c,"new",3).unwrap();let row=store::get(&c,&task).unwrap().unwrap();assert!(row.completed);assert_eq!(row.phase,"interrupted");assert_eq!(row.communication.unwrap().state,State::RequestUncertain);assert!(store::communication_write_unresolved(&c,&device).unwrap());assert!(!store::communication_write_unresolved(&c,&id()).unwrap());assert!(store::insert_with_intent(&c,&task,"new",LoginIntent::Communication,4).is_err());
}
#[test]
fn communication_old_attempt_projection_keeps_new_observation_absent(){
    let c=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&c,"owner",1).unwrap();let task=id();store::insert(&c,&task,"owner",1).unwrap();let row=store::get(&c,&task).unwrap().unwrap();assert!(row.communication.is_none());assert!(serde_json::to_value(row).unwrap().get("communication").is_none());
}

#[tokio::test]
async fn communication_queries_use_exact_request_shapes_and_current_token_without_phone_or_sdk_calls(){
    for (operation,path,body,expected) in [
        (json!({"kind":"query_profile"}),"/profile",profile_body(),Value::Null),
        (json!({"kind":"query_phone_service"}),"/phone-service",json!({"phoneServerStatus":"3"}),json!({"type":1})),
        (json!({"kind":"query_display_status"}),"/display",json!({"jegom_status":"2","region_code":"44","jegom_phone":"7700900123"}),Value::Null),
        (json!({"kind":"query_forwarding_state"}),"/state",json!({"cs_forward":"2","authentication_code":"1"}),json!({"device_id":UUID})),
    ]{
        let mut steps=app_only_steps();steps.push(complete_reply(path,body));let (f,script)=fixture(steps,false,false);let task=id();let mut r=request(&task,operation);r.write_consent=false;f.manager.submit_jegotrip(r).unwrap();let view=terminal(&f.manager,&task).await;
        assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Observed);no_native(&f,&view).await;
        assert_eq!(script.paths(),["/login","/user",path]);let seen=lock(&script.seen);assert_eq!(seen[2].body,expected);assert!(seen[0].body.get("enrollId").is_none());assert_eq!(seen[2].query["token"],"fixture-app-session");drop(seen);f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn communication_phone_service_write_uses_numeric_type_and_status_then_independent_readback(){
    let mut steps=app_only_steps();steps.push(complete_reply("/phone-service",json!({})));steps.push(complete_reply("/phone-service",json!({"phoneServerStatus":"3"})));let (f,script)=fixture(steps,true,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"set_phone_service","status":3}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Confirmed);assert_eq!(view.communication.as_ref().unwrap().status.as_ref().unwrap().phone_server_status,Some(3));no_native(&f,&view).await;
    let seen=lock(&script.seen);assert_eq!(seen[2].body,json!({"type":2,"phoneServerStatus":3}));assert_eq!(seen[3].body,json!({"type":1}));assert_eq!(seen[2].wire,seen[2].body);drop(seen);assert_eq!(script.paths(),["/login","/user","/phone-service","/phone-service"]);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_voip_profile_write_uses_string_cs_forward_and_get_call_status_not_get_profile(){
    let mut steps=app_only_steps();steps.push(complete_reply("/set-voip",json!({})));steps.push(complete_reply("/state",json!({"cs_forward":"1","authentication_code":"2"})));let (f,script)=fixture(steps,true,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"set_voip_profile","cs_forward":"1"}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Confirmed);no_native(&f,&view).await;assert_eq!(script.paths(),["/login","/user","/set-voip","/state"]);let seen=lock(&script.seen);assert_eq!(seen[2].body,json!({"cs_forward":"1"}));assert_eq!(seen[3].body,json!({"device_id":UUID}));drop(seen);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_close_forward_uses_confirmed_bound_number_and_original_cs_two_ack_not_a_hosting_disable_claim(){
    let mut steps=app_only_steps();steps.push(complete_reply("/close",json!({"cs_forward":"2"})));let (f,script)=fixture(steps,true,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"close_forward"}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Acknowledged);no_native(&f,&view).await;assert_eq!(script.paths(),["/login","/user","/close"]);let seen=lock(&script.seen);assert_eq!(seen[2].body,json!({"mobile":"7700900123","countryCode":"44"}));assert!(seen[2].wire["body"].is_string());assert!(seen[2].wire.get("sec").is_some());drop(seen);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_jegom_write_is_encrypted_and_uses_encrypted_display_readback_not_fake_request_status(){
    let mut steps=app_only_steps();steps.push(complete_reply("/jego",json!({"jegom_phone":"7700900123","region_code":"44"})));steps.push(complete_reply("/display",json!({"jegom_status":"2","region_code":"44","jegom_phone":"7700900123"})));let (f,script)=fixture(steps,true,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"set_jego_m_status","jegom_status":"2","region_code":"44","country_code":"44"}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Confirmed);no_native(&f,&view).await;assert_eq!(script.paths(),["/login","/user","/jego","/display"]);let seen=lock(&script.seen);assert_eq!(seen[2].body,json!({"jegom_status":"2","region_code":"44","country_code":"44"}));assert!(seen[2].wire["body"].is_string());assert!(seen[3].body.is_null());drop(seen);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_app_logout_sends_empty_ordinary_body_and_never_logs_out_native_or_changes_forwarding(){
    let mut steps=app_only_steps();steps.push(complete_reply("/logout",json!({})));let (f,script)=fixture(steps,true,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"logout"}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Confirmed);no_native(&f,&view).await;assert_eq!(script.paths(),["/login","/user","/logout"]);assert!(lock(&script.seen)[2].wire.is_null());f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_missing_or_malformed_read_model_never_produces_observed_success(){
    for (kind,path,body) in [("query_profile","/profile",json!({})),("query_phone_service","/phone-service",json!({"phoneServerStatus":true})),("query_display_status","/display",json!({"jegom_status":"private-token"}))]{
        let mut steps=app_only_steps();steps.push(complete_reply(path,body));let (f,_)=fixture(steps,false,false);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":kind}))).unwrap();let view=terminal(&f.manager,&task).await;assert_eq!(view.error,Some("app_login_response_invalid"));assert_eq!(view.communication.as_ref().unwrap().state,State::RequestUncertain);no_native(&f,&view).await;f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn communication_negative_readback_retains_accepted_unconfirmed_and_blocks_new_write_nonce(){
    let mut steps=app_only_steps();steps.push(complete_reply("/phone-service",json!({})));steps.push(complete_reply("/phone-service",json!({"phoneServerStatus":4})));let (f,script)=fixture(steps,false,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"set_phone_service","status":3}))).unwrap();let view=terminal(&f.manager,&task).await;idle(&f).await;
    assert_eq!(view.error,Some("app_login_communication_unconfirmed"));assert_eq!(view.communication.as_ref().unwrap().state,State::AcceptedUnconfirmed);assert_eq!(f.manager.submit_jegotrip(request(&id(),json!({"kind":"logout"}))).unwrap_err().code,"app_login_communication_unresolved");assert_eq!(script.paths(),["/login","/user","/phone-service","/phone-service"]);no_native(&f,&view).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_lost_write_result_is_durable_and_never_replayed_by_same_or_different_task_id(){
    let mut steps=app_only_steps();steps.push(Step::Fail("/phone-service".into()));let (f,script)=fixture(steps,false,true);let task=id();let op=json!({"kind":"set_phone_service","status":3});f.manager.submit_jegotrip(request(&task,op.clone())).unwrap();let view=terminal(&f.manager,&task).await;idle(&f).await;
    assert_eq!(view.error,Some("app_login_transport_failed"));assert_eq!(view.communication.as_ref().unwrap().state,State::RequestUncertain);assert_eq!(f.manager.submit_jegotrip(request(&task,op.clone())).unwrap_err().code,"app_login_attempt_exists");assert_eq!(f.manager.submit_jegotrip(request(&id(),op)).unwrap_err().code,"app_login_communication_unresolved");assert_eq!(script.paths(),["/login","/user","/phone-service"]);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_version_gate_after_write_is_not_a_license_to_reauthenticate_or_resubmit(){
    let mut steps=app_only_steps();steps.push(reply("/phone-service",json!({"code":524,"msg":"private-reflection"})));steps.push(reply("/lookup",json!({"results":[{"version":"8.9.8.0"}]})));steps.extend(app_only_steps());
    let (f,script)=fixture(steps,false,true);let mut cfg=configuration(&f,true);cfg.http.app_version=Some(Secret::from_owned("8.9.4.0".into()));cfg.http.app_version_ios_app_id=Some(Secret::from_owned("12345".into()));f.manager.configure(ConfigurationWriteRequest{expected_revision:1,configuration:cfg}).unwrap();f.manager.set_app_version("8.9.4.0").unwrap();
    let task=id();let mut r=request(&task,json!({"kind":"set_phone_service","status":3}));r.expected_configuration_revision=Some(2);f.manager.submit_jegotrip(r).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,Some("app_login_server_rejected"));assert_eq!(view.server_code,Some(524));assert_eq!(view.communication.as_ref().unwrap().state,State::Rejected);assert_eq!(script.paths(),["/login","/user","/phone-service"]);assert!(!serde_json::to_string(&view).unwrap().contains("private-reflection"));f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_read_only_version_recovery_reauthenticates_once_with_new_token_and_keeps_original_uuid(){
    let mut steps=app_only_steps();steps.push(reply("/phone-service",json!({"code":524})));steps.push(reply("/lookup",json!({"results":[{"version":"8.9.8.0"}]})));
    let mut second=app_only_steps();second[0]=complete_reply("/login",json!({"token":"fixture-new-communication-session"}));steps.extend(second);steps.push(complete_reply("/phone-service",json!({"phoneServerStatus":3})));
    let (f,script)=fixture(steps,false,true);let mut cfg=configuration(&f,true);cfg.http.app_version=Some(Secret::from_owned("8.9.4.0".into()));cfg.http.app_version_ios_app_id=Some(Secret::from_owned("12345".into()));f.manager.configure(ConfigurationWriteRequest{expected_revision:1,configuration:cfg}).unwrap();f.manager.set_app_version("8.9.4.0").unwrap();
    let task=id();let mut r=request(&task,json!({"kind":"query_phone_service"}));r.expected_configuration_revision=Some(2);r.write_consent=false;f.manager.submit_jegotrip(r).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,None);assert_eq!(view.communication.as_ref().unwrap().state,State::Observed);assert_eq!(script.paths(),["/login","/user","/phone-service","/lookup","/login","/user","/phone-service"]);
    let seen=lock(&script.seen);assert_eq!(seen[2].query["token"],"fixture-app-session");assert_eq!(seen[6].query["token"],"fixture-new-communication-session");assert!(seen[0].body.get("enrollId").is_none());assert!(seen[4].body.get("enrollId").is_none());drop(seen);
    let identities=f.manager.device_identities().unwrap();assert_eq!(identities.identities.len(),1);assert_eq!(identities.identities[0].device_id,UUID);assert_eq!(view.communication.as_ref().unwrap().device_identity_id,identities.identities[0].id);no_native(&f,&view).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_confirmation_version_failure_never_replays_the_accepted_write(){
    let mut steps=app_only_steps();steps.push(complete_reply("/phone-service",json!({})));steps.push(reply("/phone-service",json!({"code":524})));let (f,script)=fixture(steps,false,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"set_phone_service","status":3}))).unwrap();let view=terminal(&f.manager,&task).await;
    assert_eq!(view.error,Some("app_login_communication_unconfirmed"));assert_eq!(view.communication.as_ref().unwrap().state,State::AcceptedUnconfirmed);assert_eq!(view.communication.as_ref().unwrap().server_code,Some(524));assert_eq!(script.paths(),["/login","/user","/phone-service","/phone-service"]);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_shutdown_during_write_consumes_original_worker_without_compensation_or_repeat(){
    let mut steps=app_only_steps();steps.push(Step::Park("/logout".into()));let (f,script)=fixture(steps,false,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"logout"}))).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async{while script.paths().len()!=3{tokio::task::yield_now().await;}}).await.unwrap();f.manager.shutdown().await;let view=f.manager.get(&task).unwrap();assert_eq!(view.communication.as_ref().unwrap().state,State::RequestUncertain);assert_eq!(view.error,Some("app_login_interrupted"));assert_eq!(script.paths(),["/login","/user","/logout"]);assert_eq!(f.manager.active.available_permits(),2);no_native(&f,&view).await;
}
#[tokio::test]
async fn communication_observation_storage_failure_prevents_the_business_request(){
    let (f,script)=fixture(app_only_steps(),false,true);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_communication BEFORE UPDATE OF communication ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"logout"}))).unwrap();let view=terminal(&f.manager,&task).await;assert_eq!(view.error,Some("app_login_storage_failed"));assert!(view.communication.is_none());assert_eq!(script.paths(),["/login","/user"]);no_native(&f,&view).await;f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_configuration_change_while_admitted_prevents_a_business_request_using_stale_approval(){
    let (f,script)=fixture(app_only_steps(),false,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"logout"}))).unwrap();f.manager.configure(ConfigurationWriteRequest{expected_revision:1,configuration:configuration(&f,false)}).unwrap();let view=terminal(&f.manager,&task).await;assert_eq!(view.error,Some("app_login_configuration_revision_conflict"));assert!(view.communication.is_none());assert_eq!(script.paths(),["/login","/user"]);f.manager.shutdown().await;
}
#[tokio::test]
async fn communication_same_alias_cannot_submit_concurrently_or_change_original_uuid_under_the_active_attempt(){
    let (f,script)=fixture(vec![Step::Park("/login".into())],false,true);let task=id();f.manager.submit_jegotrip(request(&task,json!({"kind":"logout"}))).unwrap();assert_eq!(f.manager.submit_jegotrip(request(&id(),json!({"kind":"logout"}))).unwrap_err().code,"app_login_busy");
    let mut r=request(&id(),json!({"kind":"query_profile"}));r.device_id=Some(Secret::from_owned("F1C0199B-1644-4D33-8912-B8BD5AB49224".into()));assert_eq!(f.manager.submit_jegotrip(r).unwrap_err().code,"app_login_busy");f.manager.shutdown().await;assert!(script.paths().len()<=1);assert_eq!(f.manager.list().unwrap().len(),1);
}
