//! Actions-only tests: synthetic App credentials, SQLite, real loopback HTTPS
//! and the existing native TLS/Common peer. Never an APK or real service run.
use super::*;
use super::{config::{HttpDeployment, Deployment}, crypto::JsonDoc, http::{Transport, Exchange, WireRequest, WireResponse}};
use super::super::{account_test_support::{TempDatabase, QueuedFactory, config_json, peer}, account_store,
    tls_engine::tests::Certificates, rpc_envelope::decode_request, runtime_backend::tests::next_non_poll};
use serde_json::{json, Value};
use std::collections::{VecDeque, BTreeMap};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use zeroize::Zeroizing;

const SIP_CIPHER:&str="hjhmeor4ER9SUekKGe7UZQ==";

fn database_configuration(f:&Fixture,native:bool)->DatabaseConfiguration {
    let mut value=deployment_json(&f.cert.root,f.script.protection.is_some());
    if !native {value["runtime"]=Value::Null;}
    serde_json::from_value(value).unwrap()
}
fn app_only_request(id:&str)->JegotripLoginRequest {
    serde_json::from_value(json!({"attempt_id":id,"username":" user@example.invalid ","password":" p@ss ","country_code":null,
        "intent":"authenticate","consent":true,"start_receiver":false})).unwrap()
}
fn app_only_steps()->Vec<Step> {let mut steps=prefix();steps.truncate(2);steps}

#[test]
fn legacy_attempt_table_rename_preserves_ids_and_uncertain_fields() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE keep_fixture(value TEXT); INSERT INTO keep_fixture VALUES('unchanged');
        CREATE TABLE app_voice_login_attempts(attempt_id TEXT PRIMARY KEY,owner TEXT NOT NULL,phase TEXT NOT NULL,business_state TEXT NOT NULL,
        completed INTEGER NOT NULL,line_id TEXT,masked_number TEXT,error TEXT,server_code INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);").unwrap();
    let id=id();
    conn.execute("INSERT INTO app_voice_login_attempts VALUES(?1,'owner','confirming_business','accepted_unconfirmed',0,NULL,'***0123',NULL,NULL,10,11)",[&id]).unwrap();
    store::initialize(&conn,"owner",20).unwrap();
    let value=store::get(&conn,&id).unwrap().unwrap();
    assert_eq!(value.phase,"confirming_business");assert_eq!(value.business_state,"accepted_unconfirmed");assert!(!value.completed);
    assert_eq!((value.created_at,value.updated_at),(10,11));assert_eq!(value.intent,LoginIntent::EnableHosting);assert_eq!(value.account_id,None);
    assert!(value.device_trust.is_none());
    assert_eq!(conn.query_row("SELECT owner FROM jegotrip_login_attempts",[],|r|r.get::<_,String>(0)).unwrap(),"owner");
    assert_eq!(conn.query_row("SELECT value FROM keep_fixture",[],|r|r.get::<_,String>(0)).unwrap(),"unchanged");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='app_voice_login_attempts'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    store::initialize(&conn,"new-owner",30).unwrap();
    let interrupted=store::get(&conn,&id).unwrap().unwrap();assert_eq!(interrupted.phase,"interrupted");assert_eq!(interrupted.business_state,"accepted_unconfirmed");
    assert!(store::insert(&conn,&id,"new-owner",31).is_err());
}
#[test]
fn ambiguous_dual_ledger_migration_fails_atomically_without_dropping_records() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",1).unwrap();
    let new_id=id();store::insert(&conn,&new_id,"owner",1).unwrap();
    conn.execute_batch("CREATE TABLE app_voice_login_attempts AS SELECT * FROM jegotrip_login_attempts;").unwrap();
    assert_eq!(store::initialize(&conn,"next-owner",2),Err("app_login_migration_conflict"));
    for table in ["app_voice_login_attempts","jegotrip_login_attempts"] {
        assert_eq!(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    assert!(!store::get(&conn,&new_id).unwrap().unwrap().completed);
}
#[test]
fn database_configuration_cas_and_clear_do_not_restore_legacy_file() {
    let f=Fixture::new(vec![],true);
    let saved=f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    assert_eq!(saved.configuration_source,"database");assert_eq!(saved.revision,1);assert!(saved.accepting_authenticate);assert!(!saved.accepting_enable_hosting);
    let text=serde_json::to_string(&saved).unwrap();assert!(!text.contains("fixture-"));assert!(!text.contains("secret_value"));assert!(f.script.paths().is_empty());
    assert_eq!(f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap_err().code,"app_login_configuration_revision_conflict");
    let cleared=f.manager.clear_configuration(1).unwrap();assert_eq!(cleared.revision,2);assert!(!cleared.http_configured);
    assert!(f.path.is_file());assert!(!f.manager.setup().deployment_present);assert!(!f.manager.setup().accepting_authenticate);
    assert_eq!(f.manager.configure(ConfigurationWriteRequest {expected_revision:1,configuration:database_configuration(&f,false)}).unwrap_err().code,"app_login_configuration_revision_conflict");
}
#[tokio::test]
async fn app_only_database_login_without_file_records_summary_without_phone_or_business_requests() {
    let f=Fixture::new(app_only_steps(),true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    std::fs::remove_file(&f.path).unwrap();
    let id=id();f.manager.submit_jegotrip(app_only_request(&id)).unwrap();let view=terminal(&f.manager,&id).await;
    assert_eq!(view.phase,"completed");assert_eq!(view.intent,LoginIntent::Authenticate);assert_eq!(view.business_state,"not_requested");
    assert!(view.account_id.is_some());assert_eq!(view.line_id,None);assert_redacted(&view);
    assert_eq!(f.script.paths(),["/login","/user"]);assert_eq!(f.factory.build_count(),0);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());
    let rows=f.manager.app_accounts().unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].masked_number,"***0123");assert_eq!(rows[0].voice_line_id,None);
    let text=serde_json::to_string(&rows).unwrap();for secret in ["p@ss","fixture-app-session","7700900123","user@example.invalid"] {assert!(!text.contains(secret));}
}
#[test]
fn hosting_with_http_only_database_is_rejected_before_any_authentication() {
    let f=Fixture::new(vec![],true);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    assert_eq!(f.manager.submit(login_request(&id())).unwrap_err().code,"app_login_native_configuration_missing");
    let mut req=app_only_request(&id());req.intent=LoginIntent::EnableHosting;
    assert_eq!(f.manager.submit_jegotrip(req).unwrap_err().code,"app_login_native_configuration_missing");assert!(f.script.paths().is_empty());
}
#[test]
fn new_account_login_requires_explicit_consent_and_cannot_start_receiver_in_auth_only_mode() {
    let f=Fixture::new(vec![],false);
    let mut req=app_only_request(&id());req.consent=false;assert_eq!(f.manager.submit_jegotrip(req).unwrap_err().code,"app_login_request_invalid");
    let mut req=app_only_request(&id());req.start_receiver=true;assert_eq!(f.manager.submit_jegotrip(req).unwrap_err().code,"app_login_request_invalid");
    assert!(f.script.paths().is_empty());
}
#[tokio::test]
async fn challenged_app_only_authentication_does_not_create_account_summary_or_hosting() {
    let f=Fixture::new(vec![reply("/login",json!({"code":88001,"body":{"token":"must-not-use"}}))],true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    let id=id();f.manager.submit_jegotrip(app_only_request(&id)).unwrap();let v=terminal(&f.manager,&id).await;
    assert_eq!(v.error,Some("app_login_verification_required"));assert_eq!(v.account_id,None);assert!(f.manager.app_accounts().unwrap().is_empty());
    assert_eq!(f.script.paths(),["/login"]);assert_eq!(v.business_state,"not_requested");
}
#[tokio::test]
async fn account_summary_failure_is_observable_and_never_starts_phone_flow() {
    let f=Fixture::new(app_only_steps(),true);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_account_summary BEFORE INSERT ON jegotrip_accounts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    let id=id();f.manager.submit_jegotrip(app_only_request(&id)).unwrap();let v=terminal(&f.manager,&id).await;
    assert_eq!(v.error,Some("app_login_storage_failed"));assert_eq!(v.business_state,"not_requested");assert_eq!(f.script.paths(),["/login","/user"]);
    assert!(f.manager.app_accounts().unwrap().is_empty());
}
#[tokio::test]
async fn admitted_login_retains_configuration_snapshot_while_database_is_updated() {
    let f=Fixture::new(app_only_steps(),false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    let id=id();f.manager.submit_jegotrip(app_only_request(&id)).unwrap();
    let mut changed=database_configuration(&f,false);changed.http.anonymous_token=Secret::from_owned("changed-anonymous".into());changed.http.signature_salt=Secret::from_owned("changed-salt".into());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:1,configuration:changed}).unwrap();
    let v=terminal(&f.manager,&id).await;assert_eq!(v.phase,"completed");
    let seen=lock(&f.script.seen);let login=&seen[0];assert_eq!(login.query["n_token"],"fixture-anonymous");
    assert_eq!(login.query["sign"],crypto::url_signature(&http_config(false).signature_salt,login.query["timestamp"].parse().unwrap()));
    assert_eq!(f.manager.app_accounts().unwrap()[0].configuration_revision,1);assert_eq!(f.manager.configuration().unwrap().revision,2);
}
#[tokio::test]
async fn account_summary_deduplicates_server_binding_without_replaying_attempt_id() {
    let mut steps=app_only_steps();steps.extend(app_only_steps());let f=Fixture::new(steps,true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:database_configuration(&f,false)}).unwrap();
    let first=id();f.manager.submit_jegotrip(app_only_request(&first)).unwrap();let a=terminal(&f.manager,&first).await;
    assert_eq!(f.manager.submit_jegotrip(app_only_request(&first)).unwrap_err().code,"app_login_attempt_exists");
    tokio::time::timeout(Duration::from_secs(5),async {while f.manager.active.available_permits()!=2 {tokio::time::sleep(Duration::from_millis(1)).await;}}).await.unwrap();
    let next=id();f.manager.submit_jegotrip(app_only_request(&next)).unwrap();let b=terminal(&f.manager,&next).await;
    assert_eq!(a.account_id,b.account_id);assert_eq!(f.manager.app_accounts().unwrap().len(),1);assert_eq!(f.script.paths().len(),4);
}
#[test]
fn private_database_configuration_encoding_is_bounded_and_public_view_never_exports_values() {
    let f=Fixture::new(vec![],true);let c=database_configuration(&f,false);let bytes=c.encode_private().unwrap();
    assert!(DatabaseConfiguration::decode_private(&bytes).is_ok());assert_eq!(format!("{c:?}"),"JegotripDatabaseConfiguration(<redacted>)");
    assert!(DatabaseConfiguration::decode_private(&vec![b'x';256*1024+1]).is_err());
    let mut v:Value=serde_json::from_slice(&bytes).unwrap();v["http"]["phone_login_token"]=json!("never-import");assert!(serde_json::from_value::<DatabaseConfiguration>(v).is_err());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:c}).unwrap();
    let public=serde_json::to_string(&f.manager.setup()).unwrap();for text in ["fixture-","signature_salt","password_salt","secret_value","base_url"] {assert!(!public.contains(text));}
}

#[test]
fn omitted_jegotrip_intent_and_start_default_to_full_hosting_but_consent_stays_required() {
    let request: JegotripLoginRequest = serde_json::from_value(json!({
        "attempt_id": id(), "username": "user@example.invalid", "password": " p@ss ", "consent": true
    })).unwrap();
    assert_eq!(request.intent, LoginIntent::EnableHosting); assert!(request.start_receiver);
    assert_eq!(LoginIntent::default(), LoginIntent::EnableHosting);
    let f = Fixture::new(vec![], false);
    let request: JegotripLoginRequest = serde_json::from_value(json!({
        "attempt_id": id(), "username": "user@example.invalid", "password": " p@ss ", "consent": false
    })).unwrap();
    assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code, "app_login_request_invalid");
    assert!(f.script.paths().is_empty());
}
#[test]
fn explicit_authenticate_cannot_inherit_default_receiver_start_or_fallback_from_missing_runtime() {
    let f = Fixture::new(vec![], false);
    let request: JegotripLoginRequest = serde_json::from_value(json!({
        "attempt_id": id(), "username": "user@example.invalid", "password": " p@ss ", "intent": "authenticate", "consent": true
    })).unwrap();
    assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code, "app_login_request_invalid");
    f.manager.configure(ConfigurationWriteRequest { expected_revision: 0, configuration: database_configuration(&f, false) }).unwrap();
    let request: JegotripLoginRequest = serde_json::from_value(json!({
        "attempt_id": id(), "username": "user@example.invalid", "password": " p@ss ", "consent": true
    })).unwrap();
    assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code, "app_login_native_configuration_missing");
    assert!(f.script.paths().is_empty()); assert!(f.manager.app_accounts().unwrap().is_empty());
}
#[test]
fn native_bootstrap_runtime_template_binds_the_auto_assembled_account_config(){
    // The App-login entry must be able to carry the SAME native Common bootstrap
    // route as a stored account: the deployment template is a full ProfileConfig,
    // so nothing here may silently downgrade to the direct TLS carrier.
    let cert=Certificates::generate("IP:127.0.0.1");
    let mut value=deployment_json(&cert.root,false);
    // The SDK server address becomes the bootstrap LOGICAL entry; the
    // destination still comes only from the matched reply.
    value["runtime"]["sdk"]["server_address"]=json!("BootEntry:sarc -h 42.7 -p 98;");
    {
        let profile=&mut value["runtime"]["profile"];
        profile["route"]=json!({"router_common":{
            "reference":"Router:tls -h 127.0.0.1 -p 443 -ri 12001;",
            "anchors":[{"router_id":12001,"host":"127.0.0.1","port":443,"authority":"127.0.0.1","root_der_base64":cert.root.clone(),"enable_sni":false}],
            "access":[],"endpoint_index":0,"resolved_host_index":0,"startup_timeout_ms":5000,"attempt_timeout_ms":8000,"compress_above":null}});
        profile["authority"]=json!("");profile["enable_sni"]=json!(false);
        for key in ["domain_name","domain_id","application_id","device_info_cc"]{profile[key]=Value::Null;}
        profile["gateway_uri"]=json!("");
        profile["gateway_metadata"]=json!([]);
        profile["bootstrap_via_router"]=json!({"routing":[],"handoff_timeout_ms":10000});
    }
    let configuration: config::DatabaseConfiguration=serde_json::from_value(value).unwrap();
    let bound=configuration.runtime.unwrap().bind("fixture-bound-account",super::crypto::PhoneMaterial{
        token:super::super::credentials::Secret::from_owned("fixture-phone-token".to_owned()),
        sip_password:super::super::credentials::Secret::from_owned("fixture-SipPwd".to_owned())}).unwrap();
    assert_eq!(bound.validate(100),Ok(()));
    assert!(bound.runtime_profile().is_ok());
    // An empty URI without the native bootstrap route stays inadmissible.
    let mut direct=deployment_json(&cert.root,false);
    direct["runtime"]["profile"]["gateway_uri"]=json!("");
    let direct:config::DatabaseConfiguration=serde_json::from_value(direct).unwrap();
    let direct=direct.runtime.unwrap().bind("fixture-bound-account",super::crypto::PhoneMaterial{
        token:super::super::credentials::Secret::from_owned("fixture-phone-token".to_owned()),
        sip_password:super::super::credentials::Secret::from_owned("fixture-SipPwd".to_owned())}).unwrap();
    assert_eq!(direct.validate(100),Err("app_account_configuration_invalid"));
}
#[test]
fn full_database_private_mirror_roundtrip_retains_runtime_without_personal_login_fields() {
    let f = Fixture::new(vec![], true);
    let configuration = database_configuration(&f, true);
    let bytes = configuration.encode_private().unwrap();
    let mut expected = deployment_json(&f.cert.root, true);
    for key in ["client_config_decode_key", "reference_decode_key"] { expected["runtime"]["sdk"][key] = Value::Null; }
    for key in ["call_incoming", "call_control"] { expected["runtime"]["profile"][key] = Value::Null; }
    // The private mirror always carries the optional pinned-certificate slots.
    expected["http"]["pinned_server_certificate_der_base64"] = Value::Null;
    expected["http"]["pinned_server_public_key_der_base64"] = Value::Null;
    expected["http"]["extra_query"] = json!([]);
    expected["http"]["extra_headers"] = json!([]);
    expected["http"]["extra_body"] = json!([]);
    expected["http"]["app_version"] = Value::Null;
    expected["http"]["app_version_file"] = Value::Null;
    expected["http"]["app_version_web_url"] = Value::Null;
    expected["http"]["app_version_ios_app_id"] = Value::Null;
    expected["http"]["app_version_store"] = json!(false);
    expected["http"]["device_identity_mode"] = json!("store");
    expected["http"]["endpoints"]["verification_trustful_device"] = Value::Null;
    expected["http"]["endpoints"]["device_enroll"] = Value::Null;
    expected["http"]["endpoints"]["security_verify"] = Value::Null;
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
    let restored = DatabaseConfiguration::decode_private(&bytes).unwrap();
    assert!(restored.runtime.is_some());
    assert_eq!(restored.encode_private().unwrap().as_slice(), bytes.as_slice());
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    for key in ["phone_login_token", "account_identity", "password", "sip_password"] {
        assert!(value["runtime"]["sdk"].get(key).is_none()); assert!(value["runtime"]["hosting"].get(key).is_none());
    }
}
#[test]
fn private_database_writer_refuses_overflow_before_retaining_extra_bytes() {
    use std::io::Write;
    let mut bytes = Zeroizing::new(Vec::with_capacity(256 * 1024));
    let mut writer = config::PrivateDatabaseWriter(&mut bytes);
    assert_eq!(writer.write(&vec![b'x'; 256 * 1024]).unwrap(), 256 * 1024);
    assert!(writer.write(b"must-not-retain").is_err());
    assert_eq!(bytes.len(), 256 * 1024);
}

fn deployment_json(root:&str,encrypted:bool)->Value {
    let native=config_json(root);let mut sdk=native["credentials"]["sdk"].clone();let mut hosting=native["credentials"]["hosting"].clone();
    for key in ["account_identity","phone_login_token","tls_only","expires_at_unix_seconds"] { sdk.as_object_mut().unwrap().remove(key); }
    for key in ["sip_number","sip_password","enrollment_confirmed"] { hosting.as_object_mut().unwrap().remove(key); }
    let mut profile=native["profile"].clone();profile["incoming"]["receiver_kind"]=json!(b"Sip.Request");
    // The private mirror always carries the native bootstrap route selector;
    // the direct-carrier fixture leaves it absent.
    profile["bootstrap_via_router"]=Value::Null;
    json!({"version":1,"http":{
        "base_url":"https://app-fixture.invalid/","anonymous_token":"fixture-anonymous","signature_salt":"fixture-signing-salt",
        "password_salt":"fixture-password-salt:","sip_password_key":"0123456789abcdefABCDEFGH","device_id":"fixture-http-device",
        "endpoints":{"login":"login","user_info":"user","phone_token":"csb/token","voip_status":"voip","call_status":"state","enable_hosting":"enable"},
        "body_protection":if encrypted {json!({"secret_id":"fixture-body-id","secret_value":"fixture-body-secret"})} else {Value::Null},
        "root_der_base64":null,"user_agent":"fixture-http-agent","enroll_id":null},
        "runtime":{"sdk":sdk,"hosting":hosting,"profile":profile,"bootstrap":null,"call_media":null,"cleanup_on_disconnect":false,"reconnect":null}})
}
fn http_config(encrypted:bool)->HttpDeployment { serde_json::from_value(deployment_json("AA==",encrypted)["http"].clone()).unwrap() }
fn login_request(id:&str)->LoginRequest {
    serde_json::from_value(json!({"attempt_id":id,"username":"  user@example.invalid  ","password":" p@ss ",
        "country_code":null,"enable_hosting":true,"start_receiver":false})).unwrap()
}
fn id()->String { super::super::account_runtime::local_nonce().unwrap() }
fn reply(path:&str,body:Value)->Step { Step::Reply(path.to_owned(),200,body) }
enum Step { Reply(String,u16,Value), Fail(String), Park(String) }
#[derive(Clone)]
struct Recorded { path:String, query:BTreeMap<String,String>, body:Value, wire:Value }
struct Script { steps:Mutex<VecDeque<Step>>, seen:Mutex<Vec<Recorded>>, protection:Option<config::BodyProtection> }
impl Script {
    fn new(steps:Vec<Step>,encrypted:bool)->Self { Self {steps:Mutex::new(steps.into()),seen:Mutex::new(vec![]),protection:http_config(encrypted).body_protection} }
    fn paths(&self)->Vec<String> {lock(&self.seen).iter().map(|r|r.path.clone()).collect()}
}
impl Transport for Script {
    fn exchange(&self,request:WireRequest)->Exchange<'_> {Box::pin(async move {
        let path=request.url.path().to_owned();let encrypted=!matches!(path.as_str(),"/voip"|"/state"|"/trust") && self.protection.is_some();
        let wire=if request.body.is_empty() {Value::Null} else {serde_json::from_slice(&request.body).unwrap()};
        let payload=if request.body.is_empty() {Value::Null} else {
            let mut doc=crypto::parse(&request.body).unwrap();
            if encrypted {crypto::decrypt_body(&mut doc,self.protection.as_ref()).unwrap();doc.0["body"].clone()} else {doc.0.clone()}
        };
        lock(&self.seen).push(Recorded {path:path.clone(),query:request.url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect(),body:payload,wire});
        let step=lock(&self.steps).pop_front().expect("unexpected extra App request");
        match step {
            Step::Fail(expected)=>{assert_eq!(path,expected);Err(Error::new("app_login_transport_failed"))},
            Step::Park(expected)=>{assert_eq!(path,expected);std::future::pending().await},
            Step::Reply(expected,status,mut body)=>{
                assert_eq!(path,expected);
                if encrypted && body["body"].is_object() {
                    let plain=serde_json::to_vec(&body["body"]).unwrap();
                    let encoded=crypto::request_body(&plain,self.protection.as_ref(),"1700000000123105").unwrap();
                    let env:Value=serde_json::from_slice(&encoded).unwrap();body["body"]=env["body"].clone();body["sec"]=env["sec"].clone();
                }
                Ok(WireResponse {status,body:Zeroizing::new(serde_json::to_vec(&body).unwrap())})
            }
        }
    })}
}
struct Factory(Arc<Script>);
impl TransportFactory for Factory {fn build(&self,c:&HttpDeployment)->Result<AppClient,Error>{AppClient::new(c.clone(),self.0.clone())}}
fn prefix()->Vec<Step> {vec![
    reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0,"openId":"fixture-open"}})),
    reply("/user",json!({"code":0,"body":{"mobile":"7700900123","country_code":"44","user_id":"fixture-user-id","cToken":"not-a-phone-token"}})),
    reply("/csb/token",json!({"code":"0","body":{"token":"fixture-original-pwd"}})),
    reply("/voip",json!({"code":0,"body":{"sip_password":SIP_CIPHER,"status":"1"}})),
]}
fn normal_steps(already:bool)->Vec<Step> {
    let mut s=prefix();
    // The Android authentication_code UI flag is deliberately zero. Only the
    // actual authenticated business response grants business acceptance.
    s.push(reply("/state",json!({"code":0,"body":{"cs_forward":if already {"2"} else {"1"},"authentication_code":"0"}})));
    if !already {s.push(reply("/enable",json!({"code":0})));s.push(reply("/state",json!({"code":"0","body":{"cs_forward":"2","authentication_code":"0"}})));}
    s
}
struct Fixture { temp:TempDatabase, accounts:Arc<AccountManager>, manager:Arc<AppLoginManager>, script:Arc<Script>, factory:Arc<QueuedFactory>, cert:Certificates, path:PathBuf }
impl Fixture {
    fn new(steps:Vec<Step>,encrypted:bool)->Self {
        let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
        let accounts=Arc::new(AccountManager::with_factory(temp.database.clone(),factory.clone()));
        let mut lock_path=temp.path.as_os_str().to_os_string();lock_path.push(".lock");
        accounts.claim_process_ownership(std::path::Path::new(&lock_path)).unwrap();
        let path=temp.path.with_extension("app-login.json");std::fs::write(&path,serde_json::to_vec(&deployment_json(&cert.root,encrypted)).unwrap()).unwrap();
        let script=Arc::new(Script::new(steps,encrypted));let mut m=AppLoginManager::new(temp.database.clone(),Arc::downgrade(&accounts));
        m.factory=Arc::new(Factory(script.clone()));let manager=Arc::new(m);manager.initialize(path.clone()).unwrap();
        Self {temp,accounts,manager,script,factory,cert,path}
    }
}
impl Drop for Fixture {fn drop(&mut self){let _=std::fs::remove_file(&self.path);}}
async fn terminal(m:&AppLoginManager,id:&str)->AttemptView {
    tokio::time::timeout(Duration::from_secs(8),async {loop {let v=m.get(id).unwrap();if v.completed || v.error.is_some(){return v;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap()
}
fn assert_redacted(v:&AttemptView) {
    let text=serde_json::to_string(v).unwrap();
    for secret in ["p@ss","fixture-","7700900123","phone_login_token","sip_password","password_salt"] {assert!(!text.contains(secret),"unexpected private material");}
}

#[test]
fn original_password_hash_preserves_spaces_and_is_not_body_or_sip_key() {
    let c=http_config(false);let p=Secret::from_owned(" p@ss ".into());
    assert_eq!(crypto::password_hash(&c.password_salt,&p).expose_to_backend(),"940395cfb581dc7279b89a1024632d1d");
    assert_ne!(crypto::password_hash(&c.password_salt,&Secret::from_owned("p@ss".into())).expose_to_backend(),"940395cfb581dc7279b89a1024632d1d");
}
#[test]
fn original_url_signature_uses_eight_transformed_salt_bytes_and_big_endian_time() {
    assert_eq!(crypto::url_signature(&http_config(false).signature_salt,1700000000123),"50f3db4bab952408ca4f64d5c8a7b8c0f2e7719a");
}
#[test]
fn aes_envelope_matches_independent_fixed_vector_and_parses_decrypted_body() {
    let c=http_config(true);let plain=br#"{"loginName":"user","password":"hash","site":1,"userType":"0"}"#;
    let wire=crypto::request_body(plain,c.body_protection.as_ref(),"1700000000123105").unwrap();let mut doc=crypto::parse(&wire).unwrap();
    assert_eq!(doc.0["body"],"UDjjU/+Edg8tDIJi286dXosCAeEz91sf2KxOKF9Bqf6u+/YLCZwvwAMbBxeNNDM9S8CdLEcIaWlbEXvCkOqfMA==");
    assert_eq!(doc.0["sec"],"Zml4dHVyZS1ib2R5LWlkOzE3MDAwMDAwMDAxMjMxMDU7MDE=");
    crypto::decrypt_body(&mut doc,c.body_protection.as_ref()).unwrap();assert_eq!(doc.0["body"]["loginName"],"user");
}
#[test]
fn encrypted_responses_never_fall_back_to_plaintext_or_partially_decoded_success() {
    let c=http_config(true);
    for value in [json!({"body":{"token":"fixture"}}),json!({"body":"bad","sec":"bad"}),json!({"body":"AA==","sec":"YSNi"})] {
        assert_eq!(crypto::decrypt_body(&mut JsonDoc(value),c.body_protection.as_ref()).unwrap_err().code,"app_login_response_invalid");
    }
    assert!(crypto::parse(&vec![b' ';crypto::MAX_BODY+1]).is_err());
}
#[test]
fn sip_password_uses_separate_triple_des_context_and_strict_padding() {
    let c=http_config(false);let clear=crypto::sip_password(&Secret::from_owned(SIP_CIPHER.into()),&c).unwrap();assert_eq!(clear.expose_to_backend(),"fixture-SipPwd");
    assert!(crypto::sip_password(&Secret::from_owned("AA==".into()),&c).is_err());
}
#[test]
fn only_canonical_zero_is_login_success_and_challenges_remain_distinct() {
    for code in [json!("00"),json!("+0"),json!(false),json!(null),json!(0.0),json!({}),json!([])] {assert!(crypto::success(&JsonDoc(json!({"code":code}))).is_err());}
    for code in [json!("0"),json!(0)] {assert!(crypto::success(&JsonDoc(json!({"code":code}))).is_ok());}
    for (code,wanted) in [(88001,"app_login_verification_required"),(2006,"app_login_existing_business_action_required"),(2044,"app_login_migration_required"),(429,"app_login_session_revoked"),(9999,"app_login_service_maintenance")] {
        let e=crypto::success(&JsonDoc(json!({"code":code,"body":{"token":"do-not-adopt"}}))).unwrap_err();assert_eq!(e.code,wanted);assert_eq!(e.server_code,Some(code));
    }
}
#[test]
fn deployment_is_https_origin_pinned_and_contains_no_personal_sdk_fields() {
    let mut c=http_config(false);assert!(c.validate().is_ok());
    for endpoint in ["https://other.invalid/","//other.invalid/","../login","login?token=x","login#x","%2e%2e/login"] {assert!(c.endpoint(endpoint).is_err());}
    c.base_url=Secret::from_owned("http://invalid.example/".into());assert!(c.validate().is_err());
    let mut v=deployment_json("AA==",false);v["runtime"]["sdk"]["phone_login_token"]=json!("must-be-fetched");assert!(serde_json::from_value::<Deployment>(v).is_err());
}
#[test]
fn request_requires_explicit_business_consent_and_keeps_password_private() {
    let mut r=login_request(&id());assert!(r.validate().is_ok());assert_eq!(format!("{r:?}"),"LoginRequest(<redacted>)");
    r.enable_hosting=false;assert!(r.validate().is_err());r.enable_hosting=true;r.attempt_id="bad".into();assert!(r.validate().is_err());
    let mut r=login_request(&id());r.country_code=Some("44".into());assert!(r.validate().is_err());
}
#[tokio::test]
async fn original_account_request_is_signed_and_never_sends_plain_password_or_telemetry() {
    let script=Arc::new(Script::new(vec![reply("/login",json!({"code":0,"body":{"token":"fixture-app-session"}}))],true));
    let mut client=AppClient::new(http_config(true),script.clone()).unwrap();client.login(&login_request(&id())).await.unwrap();
    let seen=lock(&script.seen);let r=&seen[0];assert_eq!(r.body,json!({"loginName":"user@example.invalid","password":"940395cfb581dc7279b89a1024632d1d","site":1,"userType":"0"}));
    assert_eq!(r.query["n_token"],"fixture-anonymous");assert_eq!(r.query["lang"],"zh_CN");assert!(!r.query.contains_key("token"));
    let time=r.query["timestamp"].parse().unwrap();assert_eq!(r.query["sign"],crypto::url_signature(&http_config(false).signature_salt,time));
}
#[tokio::test]
async fn successful_account_flow_fetches_bound_credentials_enables_and_keeps_readiness_separate() {
    let f=Fixture::new(normal_steps(false),true);let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();
    let view=terminal(&f.manager,&id).await;assert_eq!(view.phase,"completed");assert_eq!(view.business_state,"enabled");assert_redacted(&view);
    assert_eq!(view.masked_number.as_deref(),Some("***0123"));assert_eq!(f.script.paths(),["/login","/user","/csb/token","/voip","/state","/enable","/state"]);
    let line=view.line_id.unwrap();let stored=f.temp.database.with_conn(|c|account_store::load(c,&line)).unwrap().unwrap();
    assert_eq!(stored.config.credentials.sdk.account_identity.expose_to_backend(),"+447700900123");
    assert_eq!(stored.config.credentials.sdk.phone_login_token.expose_to_backend(),"fixture-original-pwd");
    assert_eq!(stored.config.credentials.hosting.sip_password.expose_to_backend(),"fixture-SipPwd");assert_eq!(stored.config.credentials.sdk.expires_at_unix_seconds,None);
    let current=f.accounts.get(&line,now() as u64).await.unwrap();assert_eq!(current.phase,"stopped");assert!(!current.incoming_message_ready&&!current.incoming_call_ready&&!current.outbound_authorized);
    assert_eq!(f.factory.build_count(),0);
    for r in lock(&f.script.seen).iter().skip(1) {assert_eq!(r.query["token"],"fixture-app-session");assert!(!r.query.contains_key("n_token"));}
    assert_eq!(lock(&f.script.seen)[2].body,json!({"accountId":"+447700900123"}));
    assert_eq!(lock(&f.script.seen)[5].body,json!({"cs_forward":"2","device_id":"fixture-http-device"}));
    f.manager.shutdown().await;
}
#[tokio::test]
async fn already_enabled_business_does_not_repeat_mutation() {
    let f=Fixture::new(normal_steps(true),false);let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();
    let view=terminal(&f.manager,&id).await;assert_eq!(view.phase,"completed");assert_eq!(view.business_state,"already_enabled");assert!(!f.script.paths().contains(&"/enable".to_owned()));f.manager.shutdown().await;
}
#[tokio::test]
async fn challenge_and_existing_business_login_codes_never_fetch_or_close_or_enable() {
    for code in [88001,2006,2044,429,9999] {
        let f=Fixture::new(vec![reply("/login",json!({"code":code,"body":{"token":"not-success"}}))],false);
        let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let view=terminal(&f.manager,&id).await;
        assert_eq!(view.phase,"failed");assert_eq!(view.server_code,Some(code));assert_eq!(view.business_state,"not_requested");assert_eq!(f.script.paths(),["/login"]);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());assert_redacted(&view);f.manager.shutdown().await;
    }
}
#[tokio::test]
async fn wrong_bound_phone_is_rejected_before_phone_token_or_business_requests() {
    let mut steps=prefix();steps.truncate(2);let f=Fixture::new(steps,false);let mut req=login_request(&id());req.username=Secret::from_owned("7700900999".into());req.country_code=Some("44".into());
    let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let view=terminal(&f.manager,&id).await;
    assert_eq!(view.error,Some("app_login_identity_mismatch"));assert_eq!(f.script.paths(),["/login","/user"]);assert!(view.line_id.is_none());f.manager.shutdown().await;
}
#[tokio::test]
async fn native_template_validation_precedes_remote_business_write() {
    let f=Fixture::new(prefix(),false);let mut config=deployment_json(&f.cert.root,false);config["runtime"]["profile"]["handshake_timeout_ms"]=json!(0);std::fs::write(&f.path,serde_json::to_vec(&config).unwrap()).unwrap();
    let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let view=terminal(&f.manager,&id).await;
    assert_eq!(view.error,Some("app_login_native_configuration_invalid"));assert_eq!(f.script.paths().len(),4);assert_eq!(view.business_state,"not_requested");f.manager.shutdown().await;
}
#[tokio::test]
async fn uncertain_business_transport_result_is_durable_and_attempt_id_is_not_replayed() {
    let mut steps=normal_steps(false);steps.truncate(5);steps.push(Step::Fail("/enable".into()));let f=Fixture::new(steps,false);
    let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let v=terminal(&f.manager,&id).await;
    assert_eq!(v.business_state,"request_uncertain");assert_eq!(v.error,Some("app_login_transport_failed"));assert!(v.line_id.is_none());
    assert_eq!(f.manager.submit(login_request(&id)).unwrap_err().code,"app_login_attempt_exists");assert_eq!(f.script.paths().len(),6);f.manager.shutdown().await;
}
#[tokio::test]
async fn accepted_but_unconfirmed_business_is_not_account_or_native_success() {
    let mut steps=normal_steps(false);steps.pop();steps.push(reply("/state",json!({"code":0,"body":{"cs_forward":"1"}})));let f=Fixture::new(steps,false);
    let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let v=terminal(&f.manager,&id).await;
    assert_eq!(v.business_state,"accepted_unconfirmed");assert_eq!(v.error,Some("app_login_business_unconfirmed"));assert!(v.line_id.is_none());assert_eq!(f.factory.build_count(),0);f.manager.shutdown().await;
}
#[tokio::test]
async fn shutdown_interrupts_original_attempt_and_keeps_uncertain_write_without_replay() {
    let mut steps=normal_steps(false);steps.truncate(5);steps.push(Step::Park("/enable".into()));let f=Fixture::new(steps,false);
    let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();
    tokio::time::timeout(Duration::from_secs(4),async {while f.script.paths().len()<6 {tokio::task::yield_now().await;}}).await.unwrap();
    f.manager.shutdown().await;let v=f.manager.get(&id).unwrap();assert_eq!(v.phase,"interrupted");assert_eq!(v.business_state,"request_uncertain");assert!(v.line_id.is_none());assert!(!f.manager.setup().accepting);
}
#[tokio::test]
async fn final_ledger_failure_is_observable_and_never_replays_business() {
    let f=Fixture::new(normal_steps(false),false);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_login_finish BEFORE UPDATE OF completed ON jegotrip_login_attempts WHEN NEW.completed=1 BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
    let req=login_request(&id());let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let v=terminal(&f.manager,&id).await;
    assert_eq!(v.error,Some("app_login_storage_failed"));assert!(!v.completed);assert_eq!(v.business_state,"enabled");assert!(v.line_id.is_some());assert_eq!(f.script.paths().iter().filter(|p|p.as_str()=="/enable").count(),1);f.manager.shutdown().await;
}
#[test]
fn restart_marks_abandoned_rows_terminal_without_replaying_or_erasing_business_outcome() {
    let c=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&c,"old-owner",1).unwrap();let id=id();store::insert(&c,&id,"old-owner",1).unwrap();
    store::step(&c,&id,"old-owner","enabling_business",Some("request_uncertain"),2).unwrap();store::initialize(&c,"new-owner",3).unwrap();
    let v=store::get(&c,&id).unwrap().unwrap();assert!(v.completed);assert_eq!(v.phase,"interrupted");assert_eq!(v.business_state,"request_uncertain");
    assert!(store::step(&c,&id,"old-owner","completed",Some("enabled"),4).is_err());assert!(store::insert(&c,&id,"new-owner",4).is_err());
}
#[test]
fn no_process_claim_means_no_login_initialization_or_recovery() {
    let temp=TempDatabase::new();let accounts=Arc::new(AccountManager::new(temp.database.clone()));let m=AppLoginManager::new(temp.database.clone(),Arc::downgrade(&accounts));
    assert_eq!(m.initialize(temp.path.with_extension("json")).unwrap_err(),"app_login_unavailable");assert!(!m.setup().accepting);assert!(m.list().unwrap().is_empty());
}
#[tokio::test]
async fn real_native_login_and_sms_use_fetched_phone_credentials_not_app_session_token() {
    let f=Fixture::new(normal_steps(false),false);let (mut peer,_,dropped)=peer(&f.cert,&f.factory);
    let serving=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(12),async move {
        peer.authenticate_as(b"[username:+447700900123@fixture.invalid]",b"fixture-original-pwd").await;
        peer.swap(Some(4),true).await;peer.incoming_with_fields(91,b"Sip.Request",b"fixture-login-SMS",Some(b"fixture-sender"),None).await;
        let receipt=next_non_poll(&mut peer).await;assert_eq!(receipt.kind,super::super::transport_packets::CallKind::Reply);
        peer.swap(Some(3),true).await;let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method,super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
    }).await.unwrap()});
    let mut req=login_request(&id());req.start_receiver=true;let id=req.attempt_id.clone();f.manager.submit(req).unwrap();let v=terminal(&f.manager,&id).await;assert_eq!(v.phase,"completed");let line=v.line_id.unwrap();
    tokio::time::timeout(Duration::from_secs(5),async {loop {
        let state=f.accounts.get(&line,now() as u64).await.unwrap();
        if state.incoming_message_ready && f.accounts.messages(&line,10).unwrap().len()==1 {assert!(!state.outbound_authorized);break;}
        tokio::task::yield_now().await;
    }}).await.unwrap();
    f.accounts.stop(&line,1,now() as u64).await.unwrap();serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));assert_eq!(f.factory.build_count(),1);f.manager.shutdown().await;
}

#[tokio::test]
async fn database_full_default_keeps_uncertain_business_without_auth_only_success_or_replay() {
    let mut steps = normal_steps(false); steps.truncate(5); steps.push(Step::Fail("/enable".into()));
    let f = Fixture::new(steps, false);
    f.manager.configure(ConfigurationWriteRequest { expected_revision: 0, configuration: database_configuration(&f, true) }).unwrap();
    std::fs::remove_file(&f.path).unwrap();
    let attempt_id = id();
    let request = serde_json::from_value(json!({ "attempt_id": attempt_id, "username": "user@example.invalid", "password": " p@ss ", "consent": true })).unwrap();
    f.manager.submit_jegotrip(request).unwrap();
    let attempt = terminal(&f.manager, &attempt_id).await;
    assert_eq!(attempt.intent, LoginIntent::EnableHosting); assert_eq!(attempt.phase, "failed");
    assert_eq!(attempt.error, Some("app_login_transport_failed")); assert_eq!(attempt.business_state, "request_uncertain");
    assert!(attempt.account_id.is_some()); assert!(attempt.line_id.is_none());
    assert_eq!(f.manager.submit_jegotrip(serde_json::from_value(json!({ "attempt_id": attempt_id, "username": "user@example.invalid", "password": " p@ss ", "consent": true })).unwrap()).unwrap_err().code, "app_login_attempt_exists");
    assert_eq!(f.factory.build_count(), 0); assert_eq!(f.script.paths().len(), 6);
    f.manager.shutdown().await;
}
#[test]
fn linking_account_summary_and_attempt_line_rolls_back_together_on_storage_failure() {
    let f = Fixture::new(vec![], false);
    let attempt_id = id();
    let account_id = f.temp.database.with_conn(|c| accounts::record(c, &[7; 32], "f***", "***0123", 0, now()).map_err(|e| e.code)).unwrap();
    f.temp.database.with_conn(|c| {
        store::insert(c, &attempt_id, &lock(&f.manager.owner).clone().unwrap(), now())?;
        store::set_account(c, &attempt_id, &lock(&f.manager.owner).clone().unwrap(), &account_id, now())?;
        c.execute_batch("CREATE TRIGGER reject_summary_link BEFORE UPDATE OF voice_line_id ON jegotrip_accounts BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_| "fixture")
    }).unwrap();
    let owner = lock(&f.manager.owner).clone().unwrap();
    assert!(f.temp.database.with_conn(|c| store::set_line(c, &attempt_id, &owner, "fixture-line", now())).is_err());
    assert!(f.manager.get(&attempt_id).unwrap().line_id.is_none());
    assert!(f.manager.app_accounts().unwrap()[0].voice_line_id.is_none());
}

#[tokio::test]
async fn database_default_login_starts_original_receiver_and_sms_ack_with_fetched_credentials() {
    let f = Fixture::new(normal_steps(false), false);
    f.manager.configure(ConfigurationWriteRequest { expected_revision: 0, configuration: database_configuration(&f, true) }).unwrap();
    std::fs::remove_file(&f.path).unwrap();
    let (mut native, _, dropped) = peer(&f.cert, &f.factory);
    let serving = tokio::spawn(async move { tokio::time::timeout(Duration::from_secs(12), async move {
        native.authenticate_as(b"[username:+447700900123@fixture.invalid]", b"fixture-original-pwd").await;
        native.swap(Some(4), true).await;
        native.incoming_with_fields(92, b"Sip.Request", b"fixture-database-login-SMS", Some(b"fixture-sender"), None).await;
        assert_eq!(next_non_poll(&mut native).await.kind, super::super::transport_packets::CallKind::Reply);
        native.swap(Some(3), true).await;
        let message = next_non_poll(&mut native).await;
        let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        native.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await;
        assert!(native.receive().await.is_none());
    }).await.unwrap() });
    let attempt_id = id();
    let request = serde_json::from_value(json!({ "attempt_id": attempt_id, "username": "user@example.invalid", "password": " p@ss ", "consent": true })).unwrap();
    f.manager.submit_jegotrip(request).unwrap();
    let attempt = terminal(&f.manager, &attempt_id).await;
    assert_eq!(attempt.intent, LoginIntent::EnableHosting); assert_eq!(attempt.phase, "completed");
    assert_eq!(attempt.business_state, "enabled"); assert_redacted(&attempt);
    let line = attempt.line_id.unwrap();
    let account_id = attempt.account_id.unwrap();
    assert_eq!(f.manager.app_accounts().unwrap().into_iter().find(|a| a.account_id == account_id).unwrap().voice_line_id.as_deref(), Some(line.as_str()));
    tokio::time::timeout(Duration::from_secs(5), async { loop {
        let state = f.accounts.get(&line, now() as u64).await.unwrap();
        if state.incoming_message_ready && f.accounts.messages(&line, 10).unwrap().len() == 1 {
            assert!(!state.incoming_call_ready); assert!(!state.outbound_authorized); break;
        }
        tokio::task::yield_now().await;
    }}).await.unwrap();
    assert_eq!(f.script.paths(), ["/login", "/user", "/csb/token", "/voip", "/state", "/enable", "/state"]);
    f.accounts.stop(&line, 1, now() as u64).await.unwrap(); serving.await.unwrap();
    assert!(dropped.load(Ordering::SeqCst)); assert_eq!(f.factory.build_count(), 1);
    f.manager.shutdown().await;
}

#[tokio::test]
async fn database_app_only_login_uses_real_https_and_never_builds_a_native_peer() {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=listener.local_addr().unwrap().port();let server_config=cert.server_config(&rustls::version::TLS13);
    let worker=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(12),async move {
        let mut paths=vec![];
        for (expected,body) in [("/login",r#"{"code":0,"body":{"token":"fixture-live-App-session"}}"#),
            ("/user",r#"{"code":0,"body":{"mobile":"7700900123","country_code":"44"}}"#)] {
            let (tcp,_)=listener.accept().await.unwrap();let mut stream=tokio_rustls::TlsAcceptor::from(server_config.clone()).accept(tcp).await.unwrap();
            let mut request=vec![];let mut buf=[0u8;2048];
            loop {
                let n=stream.read(&mut buf).await.unwrap();assert!(n>0);request.extend_from_slice(&buf[..n]);assert!(request.len()<65536);
                if let Some(end)=request.windows(4).position(|w|w==b"\r\n\r\n") {
                    let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length=headers.lines().find_map(|line|line.strip_prefix("content-length:")).map(|s|s.trim().parse::<usize>().unwrap()).unwrap_or(0);
                    if request.len()>=end+4+length {break;}
                }
            }
            let text=String::from_utf8(request).unwrap();assert!(text.starts_with(&format!("POST {expected}?")));assert!(!text.contains("p@ss"));paths.push(expected);
            let reply=format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            stream.write_all(reply.as_bytes()).await.unwrap();let _=stream.shutdown().await;
        }
        paths
    }).await.unwrap()});
    let f=Fixture::new(vec![],false);let manager=Arc::new(AppLoginManager::new(f.temp.database.clone(),Arc::downgrade(&f.accounts)));
    manager.initialize(f.temp.path.with_extension("no-install-file.json")).unwrap();
    let mut configuration=database_configuration(&f,false);configuration.http.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));
    configuration.http.root_der_base64=Some(Secret::from_owned(cert.root.clone()));
    manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration}).unwrap();let id=id();manager.submit_jegotrip(app_only_request(&id)).unwrap();
    let view=terminal(&manager,&id).await;assert_eq!(view.phase,"completed");assert_eq!(view.line_id,None);assert_eq!(view.business_state,"not_requested");assert_redacted(&view);
    assert_eq!(worker.await.unwrap(),["/login","/user"]);assert_eq!(manager.app_accounts().unwrap().len(),1);assert_eq!(f.factory.build_count(),0);
    manager.shutdown().await;
}

async fn https_once(config:Arc<rustls::ServerConfig>,response:String)->(u16,tokio::task::JoinHandle<Option<Vec<u8>>>) {
    let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();
    let task=tokio::spawn(async move {tokio::time::timeout(Duration::from_secs(8),async move {
        let (tcp,_)=listener.accept().await.unwrap();let acceptor=tokio_rustls::TlsAcceptor::from(config);
        let Ok(mut stream)=acceptor.accept(tcp).await else {return None;};
        let mut request=vec![];let mut buf=[0u8;2048];
        loop {
            let n=stream.read(&mut buf).await.unwrap_or(0);if n==0 {return None;}request.extend_from_slice(&buf[..n]);assert!(request.len()<65536);
            if let Some(end)=request.windows(4).position(|w|w==b"\r\n\r\n") {
                let headers=String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length=headers.lines().find_map(|l|l.strip_prefix("content-length:")).map(|s|s.trim().parse::<usize>().unwrap()).unwrap_or(0);
                if request.len()>=end+4+length {break;}
            }
        }
        stream.write_all(response.as_bytes()).await.unwrap();let _=stream.shutdown().await;Some(request)
    }).await.unwrap()});(port,task)
}
#[tokio::test]
async fn live_https_transport_posts_signed_json_with_pinned_root_and_no_redirect_following() {
    let cert=Certificates::generate("IP:127.0.0.1");let body=r#"{"code":0,"body":{"token":"fixture-app-session"}}"#;
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())).await;
    let mut cfg=http_config(false);cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));cfg.root_der_base64=Some(Secret::from_owned(cert.root.clone()));
    let mut client=LiveFactory.build(&cfg).unwrap();client.login(&login_request(&id())).await.unwrap();let request=task.await.unwrap().unwrap();let text=String::from_utf8(request).unwrap();
    assert!(text.starts_with("POST /login?n_token=fixture-anonymous&lang=zh_CN&timestamp="));assert!(text.contains("&sign="));assert!(!text.contains("p@ss"));assert!(text.to_lowercase().contains("content-type: application/json; charset=utf-8"));
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),"HTTP/1.1 302 Found\r\nLocation: https://must-not-follow.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()).await;
    cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));let mut client=LiveFactory.build(&cfg).unwrap();assert_eq!(client.login(&login_request(&id())).await.unwrap_err().code,"app_login_transport_failed");assert!(task.await.unwrap().is_some());
}
#[tokio::test]
async fn pinned_server_certificate_accepts_only_the_configured_leaf() {
    // The App bundles one concrete leaf (assets/app2.cer) instead of a CA, so an
    // explicit pin is the faithful transport trust decision here.
    let cert=Certificates::generate("IP:127.0.0.1");let other=Certificates::generate("IP:127.0.0.1");
    let body=r#"{"code":0,"body":{"token":"fixture-live-App-session"}}"#;
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())).await;
    let mut cfg=http_config(false);cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));
    cfg.root_der_base64=None;cfg.pinned_server_certificate_der_base64=Some(Secret::from_owned(cert.root.clone()));
    cfg.pinned_server_public_key_der_base64=Some(Secret::from_owned(cert.public_key.clone()));
    let mut client=LiveFactory.build(&cfg).unwrap();client.login(&login_request(&id())).await.unwrap();
    assert!(task.await.unwrap().is_some());
    // Any other presented certificate is refused before a login body is delivered.
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),String::new()).await;
    let mut cfg=http_config(false);cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));
    cfg.root_der_base64=None;cfg.pinned_server_certificate_der_base64=Some(Secret::from_owned(other.root.clone()));
    cfg.pinned_server_public_key_der_base64=Some(Secret::from_owned(other.public_key.clone()));
    let mut client=LiveFactory.build(&cfg).unwrap();
    assert_eq!(client.login(&login_request(&id())).await.unwrap_err().code,"app_login_transport_failed");
    assert!(task.await.unwrap().is_none());
}
#[test]
fn app_version_can_be_static_or_reread_from_a_bounded_file_without_a_rebuild() {
    let cert=Certificates::generate("IP:127.0.0.1");
    let mut value=deployment_json(&cert.root,false);
    value["http"]["app_version"]=json!("8.9.8.0");
    assert_eq!(serde_json::from_value::<config::Deployment>(value.clone()).unwrap().http.validate(),Ok(()));
    // Both sources at once is ambiguous; malformed values and relative or
    // traversing paths are refused before any request is built.
    let mut both=value.clone();both["http"]["app_version_file"]=json!("/opt/simadmin/jegotrip-app-version.txt");
    assert!(serde_json::from_value::<config::Deployment>(both).unwrap().http.validate().is_err());
    for bad in ["","8.9.","8..9","v8.9.8","8.9.8.0 ","a".repeat(25).as_str()] {
        let mut bad_value=value.clone();bad_value["http"]["app_version"]=json!(bad);
        assert!(serde_json::from_value::<config::Deployment>(bad_value).unwrap().http.validate().is_err(),"{bad}");
    }
    let mut file=value.clone();file["http"]["app_version"]=Value::Null;
    for path in ["","relative/version.txt","/opt/simadmin/../etc/passwd","/opt/simadmin/./v"] {
        file["http"]["app_version_file"]=json!(path);
        assert!(serde_json::from_value::<config::Deployment>(file.clone()).unwrap().http.validate().is_err(),"{path}");
    }
    file["http"]["app_version_file"]=json!("/opt/simadmin/jegotrip-app-version.txt");
    assert_eq!(serde_json::from_value::<config::Deployment>(file).unwrap().http.validate(),Ok(()));
}
#[test]
fn official_page_version_discovery_picks_the_newest_8x_literal_and_never_guesses() {
    let page="<html><head><title>8.9.8</title></head><body>\n    <p>安卓版 8.9.10 发布</p>\n    <a href=\"/dl/8.9.9.apk\">download</a>\n    <span>20.1.3</span><em>1.2</em></body></html>";
    assert_eq!(http::pick_version(page).as_deref(),Some("8.9.10"));
    assert_eq!(http::pick_version("no version here").as_deref(),None);
    assert_eq!(http::pick_version("v8.x.1 and 8..9 and 8.9.").as_deref(),None);
    assert_eq!(http::pick_version("version: 8.9.8.0").as_deref(),Some("8.9.8.0"));
}
#[test]
fn official_page_url_is_restricted_to_the_official_https_host() {
    assert!(config::valid_version_page("https://www.jegotrip.cn/m/mobileIndex.html"));
    assert!(config::valid_version_page("https://jegotrip.cn/m/mobileIndex.html"));
    for bad in ["http://www.jegotrip.cn/m/mobileIndex.html","https://evil.example/m","https://jegotrip.cn.evil.example/m",
                "https://user@evil.example/m","ftp://www.jegotrip.cn/m","https://www.jegotrip.cn\n/m"] {
        assert!(!config::valid_version_page(bad),"{bad}");
    }
}
#[test]
fn extra_query_and_headers_are_bounded_and_never_replace_reserved_parameters() {
    let cert=Certificates::generate("IP:127.0.0.1");
    let mut value=deployment_json(&cert.root,false);
    value["http"]["extra_query"]=json!([{"name":"version","value":"8.9.8.0"}]);
    value["http"]["extra_headers"]=json!([{"name":"X-Client-Version","value":"8.9.8.0"}]);
    let ok:config::Deployment=serde_json::from_value(value.clone()).unwrap();
    assert_eq!(ok.http.validate(),Ok(()));
    // Reserved query names are transport-critical and cannot be overridden.
    for reserved in ["token","timestamp","sign","lang","n_token"] {
        let mut bad=value.clone();bad["http"]["extra_query"]=json!([{"name":reserved,"value":"x"}]);
        assert!(serde_json::from_value::<config::Deployment>(bad).unwrap().http.validate().is_err(),"{reserved}");
    }
    // Header/query names stay a bounded token; values stay printable single-line.
    let long_name="a".repeat(65);let long_value="a".repeat(257);
    for bad_name in ["bad name","bad:name","",long_name.as_str()] {
        let mut bad=value.clone();bad["http"]["extra_headers"]=json!([{"name":bad_name,"value":"x"}]);
        assert!(serde_json::from_value::<config::Deployment>(bad).unwrap().http.validate().is_err(),"{bad_name}");
    }
    for bad_value in ["","line\nbreak",long_value.as_str()] {
        let mut bad=value.clone();bad["http"]["extra_headers"]=json!([{"name":"X","value":bad_value}]);
        assert!(serde_json::from_value::<config::Deployment>(bad).unwrap().http.validate().is_err(),"{bad_value}");
    }
}
#[test]
fn pinned_server_certificate_is_exclusive_with_root_and_must_be_der() {
    let cert=Certificates::generate("IP:127.0.0.1");
    let mut value=deployment_json(&cert.root,false);
    value["http"]["pinned_server_certificate_der_base64"]=json!(cert.root.clone());
    value["http"]["pinned_server_public_key_der_base64"]=json!(cert.public_key.clone());
    let config:config::Deployment=serde_json::from_value(value.clone()).unwrap();
    assert_eq!(config.http.validate(),Ok(()));
    // A pin without its public key (needed for the v1 certificate) is invalid.
    let mut no_key=value.clone();no_key["http"]["pinned_server_public_key_der_base64"]=Value::Null;
    assert!(serde_json::from_value::<config::Deployment>(no_key).unwrap().http.validate().is_err());
    let mut both=value.clone();both["http"]["root_der_base64"]=json!(cert.root.clone());
    assert!(serde_json::from_value::<config::Deployment>(both).unwrap().http.validate().is_err());
    let mut bad=value.clone();bad["http"]["pinned_server_certificate_der_base64"]=json!("not-base64");
    assert!(serde_json::from_value::<config::Deployment>(bad).unwrap().http.validate().is_err());
    let mut short=value;short["http"]["pinned_server_certificate_der_base64"]=json!("MA==");
    assert!(serde_json::from_value::<config::Deployment>(short).unwrap().http.validate().is_err());
}
#[tokio::test]
async fn wrong_https_root_refuses_before_any_login_body_is_delivered() {
    let cert=Certificates::generate("IP:127.0.0.1");let other=Certificates::generate("IP:127.0.0.1");
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),String::new()).await;
    let mut cfg=http_config(false);cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));cfg.root_der_base64=Some(Secret::from_owned(other.root.clone()));
    let mut client=LiveFactory.build(&cfg).unwrap();assert_eq!(client.login(&login_request(&id())).await.unwrap_err().code,"app_login_transport_failed");assert!(task.await.unwrap().is_none());
}

#[test]
fn ios_lookup_reports_only_a_validated_8x_version_and_never_guesses() {
    assert_eq!(http::pick_ios_version(r#"{"resultCount":1,"results":[{"version":"8.9.8"}]}"#).as_deref(),Some("8.9.8"));
    assert_eq!(http::pick_ios_version(r#"{"resultCount":1,"results":[{"version":"8.9.8.0"}]}"#).as_deref(),Some("8.9.8.0"));
    // A different major, a missing/empty result set, a malformed literal and a
    // non-JSON body all resolve to "no value" instead of a guess.
    for text in [r#"{"resultCount":1,"results":[{"version":"9.0.0"}]}"#,r#"{"resultCount":0,"results":[]}"#,
                 r#"{"results":[{"version":"8.x"}]}"#,r#"{"results":[]}"#,"not json",""] {
        assert_eq!(http::pick_ios_version(text),None,"{text}");
    }
}
#[test]
fn ios_lookup_source_is_limited_to_a_numeric_app_store_id_and_explicit_store_optin() {
    let cert=Certificates::generate("IP:127.0.0.1");
    let mut value=deployment_json(&cert.root,false);
    value["http"]["app_version_ios_app_id"]=json!("1037589370");
    value["http"]["app_version_store"]=json!(true);
    let ok:config::Deployment=serde_json::from_value(value.clone()).unwrap();
    assert_eq!(ok.http.validate(),Ok(()));
    for bad in ["","0","1037589370a","../../../etc/passwd","1037589370123","https://itunes.apple.com/lookup?id=1"] {
        let mut bad_value=value.clone();bad_value["http"]["app_version_ios_app_id"]=json!(bad);
        assert!(serde_json::from_value::<config::Deployment>(bad_value).unwrap().http.validate().is_err(),"{bad}");
    }
}
#[test]
fn shared_state_stores_the_app_version_with_provenance_and_rejects_garbage() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    super::app_version::initialize(&conn).unwrap();
    // A fresh installation starts empty: no invented default version.
    assert!(super::app_version::load(&conn).is_none());
    let stored=super::app_version::manual("8.9.8.0",7).unwrap();
    assert_eq!(stored.source,super::app_version::SOURCE_MANUAL);
    super::app_version::save(&conn,&stored).unwrap();
    assert_eq!(super::app_version::load(&conn),Some(stored));
    // Overrides go through the same validation as discovered values.
    for bad in ["","8.x","8..9","v8.9.8","a".repeat(25).as_str()] {
        assert!(super::app_version::manual(bad,1).is_err(),"{bad}");
    }
    // A row that no longer holds a valid version is ignored, never trusted.
    conn.execute("UPDATE jegotrip_shared_state SET value='' WHERE name=?1",
        rusqlite::params![super::app_version::APP_VERSION_KEY]).unwrap();
    assert!(super::app_version::load(&conn).is_none());
}
#[test]
fn version_view_exposes_only_the_value_provenance_and_sources() {
    let stored=super::app_version::StoredVersion{version:"8.9.8.0".into(),source:"ios".into(),observed_at:1_000};
    let view=super::app_version::view(Some(stored),1_500,false,true,true,false,true);
    let text=serde_json::to_string(&view).unwrap();
    assert!(text.contains("\"version\":\"8.9.8.0\""),"{text}");
    assert!(text.contains("\"source\":\"ios\""),"{text}");
    assert!(text.contains("\"stored_age_seconds\":500"),"{text}");
    assert!(text.contains("\"configured_ios_lookup\":true"),"{text}");
    assert!(text.contains("\"store_enabled\":true"),"{text}");
    // Empty state stays explicit rather than fabricating a version.
    let empty=serde_json::to_string(&super::app_version::view(None,1_500,true,false,false,true,false)).unwrap();
    assert!(empty.contains("\"stored\":null"),"{empty}");
    assert!(empty.contains("\"stored_age_seconds\":null"),"{empty}");
}
#[test]
fn device_trust_codes_preserve_the_original_exact_success_literal_and_reject_padded_or_reflected_values() {
    for value in [json!("01"),json!("000001"),json!("+1"),json!(" 1"),json!("1 "),json!(1.0),json!(true),json!(-1),json!(1000000),json!("uuid-1")] {
        let body=json!({"authentication_code":value});
        assert_eq!(http::trust_code(body.as_object().unwrap()),None,"noncanonical trust value must remain unknown");
    }
    for value in [json!(1),json!("1")] {
        let body=json!({"authentication_code":value});
        assert_eq!(http::trust_code(body.as_object().unwrap()),Some(1));
    }
    for value in [json!(2),json!("2")] {
        let body=json!({"authentication_code":value});
        assert_eq!(http::trust_code(body.as_object().unwrap()),Some(2));
    }
}
#[tokio::test]
async fn trusted_device_verification_posts_the_app_fields_and_never_invents_a_grant() {
    let mut value=deployment_json("AA==",false);
    value["http"]["endpoints"]["verification_trustful_device"]=json!("trust");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
        reply("/trust",json!({"code":0,"body":{"code":0,"authentication_code":1}})),
    ],false));
    let mut client=Factory(script.clone()).build(&cfg).unwrap();
    client.login(&login_request(&id())).await.unwrap();
    // Existing-device fallback has no genuine MobileAuth credential; the App
    // session is query authentication only, never substituted into sso_token.
    assert_eq!(client.verify_trusted_device().await.unwrap(),
        Some(http::TrustDecision {authentication_code:Some(1)}));
    let seen=lock(&script.seen);
    assert_eq!(seen[1].path,"/trust");
    assert_eq!(seen[1].body["sso_token"],"");
    assert_eq!(seen[1].body["equipment_type"],"1");
    assert_eq!(seen[1].body["device_id"],"fixture-http-device");
    assert!(seen[1].query.contains_key("token"),"the App session token is query-authenticated");
}
#[tokio::test]
async fn trusted_device_verification_reports_unknown_instead_of_trust_when_the_field_is_absent() {
    let mut value=deployment_json("AA==",false);
    value["http"]["endpoints"]["verification_trustful_device"]=json!("trust");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
        reply("/trust",json!({"code":0,"body":{"code":0}})),
        reply("/trust",json!({"code":0,"body":{"authentication_code":"1"}})),
    ],false));
    let mut client=Factory(script.clone()).build(&cfg).unwrap();
    client.login(&login_request(&id())).await.unwrap();
    // A body without the field stays unknown; it is never promoted to a grant.
    assert_eq!(client.verify_trusted_device().await.unwrap(),
        Some(http::TrustDecision {authentication_code:None}));
    // A numeric string is accepted exactly like the original App's parser.
    assert_eq!(client.verify_trusted_device().await.unwrap(),
        Some(http::TrustDecision {authentication_code:Some(1)}));
}
#[tokio::test]
async fn trusted_device_verification_is_optional_and_sends_nothing_when_unconfigured() {
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
    ],false));
    let mut client=Factory(script.clone()).build(&http_config(false)).unwrap();
    client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.verify_trusted_device().await.unwrap(),None);
    assert_eq!(script.paths(),vec!["/login".to_owned()]);
}
#[tokio::test]
async fn security_device_enrolment_and_verification_send_only_the_app_device_field() {
    let mut value=deployment_json("AA==",false);
    value["http"]["endpoints"]["device_enroll"]=json!("enroll");
    value["http"]["endpoints"]["security_verify"]=json!("security");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
        reply("/enroll",json!({"code":0,"body":{"authentication_code":2}})),
        reply("/security",json!({"code":0,"body":{"authentication_code":"1"}})),
    ],false));
    let mut client=Factory(script.clone()).build(&cfg).unwrap();
    client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.enroll_device().await.unwrap(),Some(http::TrustDecision {authentication_code:Some(2)}));
    assert_eq!(client.security_verify_device().await.unwrap(),Some(http::TrustDecision {authentication_code:Some(1)}));
    let seen=lock(&script.seen);
    assert_eq!(seen[1].path,"/enroll");
    assert_eq!(seen[2].path,"/security");
    for recorded in [&seen[1],&seen[2]] {
        // The App posts exactly one field for both calls; nothing else is invented.
        assert_eq!(recorded.body,json!({"deviceId":"fixture-http-device"}));
    }
    // Enrolment precedes the App session, verification follows it.
    assert!(seen[1].query.contains_key("n_token"));
    assert!(seen[2].query.contains_key("token"));
}
#[tokio::test]
async fn security_device_calls_are_optional_and_send_nothing_when_unconfigured() {
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
    ],false));
    let mut client=Factory(script.clone()).build(&http_config(false)).unwrap();
    client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.enroll_device().await.unwrap(),None);
    assert_eq!(client.security_verify_device().await.unwrap(),None);
    assert_eq!(script.paths(),vec!["/login".to_owned()]);
}
#[tokio::test]
async fn device_enrolment_id_is_kept_for_the_next_login_body_only() {
    let mut value=deployment_json("AA==",false);
    value["http"]["endpoints"]["device_enroll"]=json!("enroll");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/enroll",json!({"code":0,"body":{"enrollId":"fixture-enroll-0001","expireTime":"2030-01-01 00:00:00"}})),
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
    ],false));
    let mut client=Factory(script.clone()).build(&cfg).unwrap();
    client.ensure_device_enrollment().await;
    client.login(&login_request(&id())).await.unwrap();
    let seen=lock(&script.seen);
    assert_eq!(seen[0].path,"/enroll");
    assert_eq!(seen[0].body,json!({"deviceId":"fixture-http-device"}));
    // Enrolment happens at login time, before any App session exists, so it is
    // the anonymous token - exactly like the original LoginActivity call.
    assert!(seen[0].query.contains_key("n_token"));
    assert!(!seen[0].query.contains_key("token"));
    // The App enrols first and then sends the received id in the login body.
    assert_eq!(seen[1].path,"/login");
    assert_eq!(seen[1].body["enrollId"],"fixture-enroll-0001");
}
#[tokio::test]
async fn attach_hosting_never_writes_the_business_switch_and_fails_closed_when_it_is_off() {
    // login, user, csb/token, voip, then the read-only business state.
    let mut steps=prefix();
    steps.push(reply("/state",json!({"code":0,"body":{"cs_forward":"1","authentication_code":"0"}})));
    let f=Fixture::new(steps,false);
    let configuration=database_configuration(&f,true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration}).unwrap();
    let id=id();
    let request:JegotripLoginRequest=serde_json::from_value(json!({"attempt_id":id,"username":" user@example.invalid ",
        "password":" p@ss ","country_code":null,"intent":"attach_hosting","consent":true,"start_receiver":true})).unwrap();
    f.manager.submit_jegotrip(request).unwrap();
    let view=terminal(&f.manager,&id).await;
    // Taking over is allowed to fail, but it must never touch the switch.
    assert_eq!(view.error.as_deref(),Some("app_login_business_not_enabled"));
    assert_eq!(view.business_state,"not_requested");
    assert_eq!(f.script.paths(),vec!["/login".to_owned(),"/user".to_owned(),"/csb/token".to_owned(),"/voip".to_owned(),"/state".to_owned()]);
    // Only the App authentication summary exists; no hosting line is created and
    // no business switch was written.
    let accounts=f.manager.app_accounts().unwrap();
    assert_eq!(accounts.len(),1);
    assert_eq!(accounts[0].voice_line_id,None);
    assert_redacted(&view);
}
#[test]
fn device_identity_is_generated_once_per_account_and_never_shared() {
    use super::device_identity as device;
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    accounts::initialize(&conn).unwrap();device::initialize(&conn).unwrap();
    let key=|value:&str|ring::digest::digest(&ring::digest::SHA256,value.as_bytes()).as_ref().to_vec();
    let reserve=|alias:&[u8],preferred:Option<&str>| {
        let tx=rusqlite::Transaction::new_unchecked(&conn,rusqlite::TransactionBehavior::Immediate).unwrap();
        let value=device::resolve(&tx,alias,preferred,None,100).unwrap();tx.commit().unwrap();value
    };
    let first=reserve(&key("account-a"),None);let second=reserve(&key("account-b"),None);
    assert_ne!(first.device_id.expose_to_backend(),second.device_id.expose_to_backend());
    assert_eq!(reserve(&key("account-a"),None).id,first.id);
    let value=first.device_id.expose_to_backend();assert_eq!(value.len(),36);
    assert!(value.chars().all(|c|c.is_ascii_digit()||c=='-'||('A'..='F').contains(&c)));
    device::set(&conn,None,0,Some(&key("number-c")),Some("PHONE-DEVICE-0001"),103).unwrap();
    let provisional=reserve(&key("account-c"),None);
    let tx=rusqlite::Transaction::new_unchecked(&conn,rusqlite::TransactionBehavior::Immediate).unwrap();
    let manual=device::confirm(&tx,&key("account-c"),&provisional,&key("number-c"),104).unwrap();tx.commit().unwrap();
    assert_eq!(manual.device_id.expose_to_backend(),"PHONE-DEVICE-0001");
    assert_eq!(reserve(&key("account-c"),None).id,manual.id);
    assert_eq!(device::set(&conn,None,0,Some(&key("number-a")),Some("PHONE-DEVICE-0001"),105).unwrap_err().code,"app_login_device_identity_conflict");
    device::set(&conn,None,0,None,Some("POOL-VALUE-0001"),106).unwrap();
    // A prepared value is used only when explicitly selected, not handed to a
    // random unrelated login from a global free pool.
    let selected=reserve(&key("account-d"),Some("POOL-VALUE-0001"));
    assert_eq!(selected.device_id.expose_to_backend(),"POOL-VALUE-0001");
    let rows=device::list(&conn).unwrap();assert!(rows.iter().any(|r|r.device_id=="PHONE-DEVICE-0001" && r.source=="manual"));
    device::remove(&conn,&selected.id,selected.revision,107).unwrap();
    assert_eq!(device::remove(&conn,&selected.id,selected.revision,108).unwrap_err().code,"app_login_device_identity_revision_conflict");
}
#[test]
fn device_identity_values_and_modes_are_validated() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    super::device_identity::initialize(&conn).unwrap();
    for bad in ["","short","has space","bad:colon","x".repeat(65).as_str()] {
        assert!(super::device_identity::set(&conn,None,0,None,Some(bad),1).is_err(),"{bad}");
    }
    for good in ["PHONE-DEVICE-0001","0123456789abcdef","A_B-C_09"] {
        assert!(super::device_identity::valid_device_id(good),"{good}");
    }
    let mut value=deployment_json("AA==",false);
    value["http"].as_object_mut().unwrap().remove("device_id");
    let default:config::HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    assert_eq!(default.device_identity_mode,config::DeviceIdentityMode::Store);assert_eq!(default.validate(),Ok(()));
    value["http"]["device_identity_mode"]=json!("static");
    let missing:config::HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();assert!(missing.validate().is_err());
    value["http"]["device_id"]=json!("fixture-device-id");
    let strict:config::HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    assert_eq!(strict.device_identity_mode,config::DeviceIdentityMode::Static);assert_eq!(strict.validate(),Ok(()));
    value["http"]["device_identity_mode"]=json!("sometimes");
    assert!(serde_json::from_value::<config::HttpDeployment>(value["http"].clone()).is_err());
}
#[tokio::test]
async fn resolved_device_identity_overrides_the_static_value_for_every_app_call() {
    let mut value=deployment_json("AA==",false);
    value["http"]["endpoints"]["device_enroll"]=json!("enroll");
    value["http"]["endpoints"]["security_verify"]=json!("security");
    value["http"]["endpoints"]["verification_trustful_device"]=json!("trust");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/enroll",json!({"code":0,"body":{"enrollId":"fixture-enroll-0009","expireTime":"2030-01-01 00:00:00"}})),
        reply("/login",json!({"code":"0","body":{"token":"fixture-app-session","expiredAt":0}})),
        reply("/security",json!({"code":0,"body":{"authentication_code":2}})),
        reply("/trust",json!({"code":0,"body":{"authentication_code":2}})),
        reply("/state",json!({"code":0,"body":{"cs_forward":"1"}})),
        reply("/enable",json!({"code":506,"body":{}})),
    ],false));
    let mut client=Factory(script.clone()).build(&cfg).unwrap().with_device_id("PHONE-DEVICE-0001");
    client.ensure_device_enrollment().await;client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.security_verify_device().await.unwrap().unwrap().authentication_code,Some(2));
    assert_eq!(client.verify_trusted_device().await.unwrap().unwrap().authentication_code,Some(2));
    assert!(!client.business_enabled().await.unwrap());assert_eq!(client.enable_business().await.unwrap_err().server_code,Some(506));
    let seen=lock(&script.seen);
    for index in [0,2] {assert_eq!(seen[index].body["deviceId"],"PHONE-DEVICE-0001");}
    for index in [3,4,5] {assert_eq!(seen[index].body["device_id"],"PHONE-DEVICE-0001");}
}

#[tokio::test]
async fn existing_device_fallback_is_plain_json_and_never_substitutes_the_app_token() {
    let mut value=deployment_json("AA==",true);
    value["http"]["endpoints"]["verification_trustful_device"]=json!("trust");
    let cfg:HttpDeployment=serde_json::from_value(value["http"].clone()).unwrap();
    let script=Arc::new(Script::new(vec![
        reply("/login",json!({"code":0,"body":{"token":"fixture-app-session"}})),
        reply("/trust",json!({"code":0,"body":{"authentication_code":2}})),
    ],true));
    let mut client=Factory(script.clone()).build(&cfg).unwrap();client.login(&login_request(&id())).await.unwrap();
    assert_eq!(client.verify_trusted_device().await.unwrap().unwrap().authentication_code,Some(2));
    let seen=lock(&script.seen);
    assert!(seen[0].wire.get("sec").is_some(),"the original login remains encrypted");
    assert_eq!(seen[1].wire,json!({"sso_token":"","equipment_type":"1","device_id":"fixture-http-device"}));
    assert_eq!(seen[1].query.get("token").map(String::as_str),Some("fixture-app-session"));
    assert!(seen[1].wire.get("sec").is_none());
    assert!(!serde_json::to_string(&seen[1].wire).unwrap().contains("fixture-app-session"));
}
#[tokio::test]
async fn existing_device_fallback_still_requires_the_owned_app_session_and_sends_nothing_without_it() {
    let mut cfg=http_config(false);cfg.endpoints.verification_trustful_device=Some("trust".into());
    let script=Arc::new(Script::new(vec![],false));let client=Factory(script.clone()).build(&cfg).unwrap();
    assert_eq!(client.verify_trusted_device().await.unwrap_err().code,"app_login_unavailable");
    assert!(script.paths().is_empty());
}
#[test]
fn push_diagnostics_do_not_promote_retained_history_or_configuration_to_live_delivery_proof() {
    let observations=push_status::PushStoredObservations {app_sms_records:Some(300),app_call_records:Some(4),
        retained_inbox_records:Some(20),pending_sms_notifications:Some(2)};
    let template=push_status::PushTemplateStatus {runtime_template_present:true,sms_receiver_present:true,sip_request_receiver_selected:true,
        call_receiver_present:true,call_control_present:true,call_media_present:true,reconnect_configured:true};
    let report=PushStatusView::assembled(true,100,template,Vec::new(),observations);
    assert!(report.warnings.contains(&"no_armed_sms_receiver"));assert!(report.warnings.contains(&"no_armed_call_receiver"));
    assert!(report.warnings.contains(&"native_accounts_absent"));
    assert_eq!(report.external_delivery_verification,"requires_external_sms_call_test");
    assert_eq!(report.receiver_namespace_verification,"requires_original_binding_evidence");
    assert!(!report.implementation.platform_offline_wakeup&&!report.implementation.call_notification_forwarding);
    assert!(report.implementation.online_native_sms&&report.implementation.online_native_calls&&report.implementation.sms_notification_forwarding);
}
#[test]
fn push_diagnostics_retain_unknown_schema_and_read_counts_without_exposing_content() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();
    let unknown=push_status::stored_observations(&conn).unwrap();assert_eq!(unknown.app_sms_records,None);assert_eq!(unknown.app_call_records,None);
    conn.execute_batch("CREATE TABLE sms_messages(transport TEXT,content TEXT);
        INSERT INTO sms_messages VALUES('app_voice','PRIVATE-SMS'),('cs','PRIVATE-PHYSICAL-SMS');
        CREATE TABLE call_history(app_call_token TEXT,phone_number TEXT);
        INSERT INTO call_history VALUES('PRIVATE-TICKET','PRIVATE-CALLER'),(NULL,'PHYSICAL-CALLER');
        CREATE TABLE app_voice_inbox(id INTEGER);INSERT INTO app_voice_inbox VALUES(1);
        CREATE TABLE app_voice_notification_jobs(state TEXT);INSERT INTO app_voice_notification_jobs VALUES('pending'),('delivered');").unwrap();
    let observations=push_status::stored_observations(&conn).unwrap();assert_eq!(observations.app_sms_records,Some(1));
    assert_eq!(observations.app_call_records,Some(1));assert_eq!(observations.pending_sms_notifications,Some(1));
    let text=serde_json::to_string(&observations).unwrap();assert!(!text.contains("PRIVATE")&&!text.contains("PHYSICAL"));
    conn.execute_batch("DROP TABLE call_history;CREATE TABLE call_history(private_unrelated_column TEXT);").unwrap();
    assert_eq!(push_status::stored_observations(&conn).err(),Some("app_login_storage_failed"));
}
#[tokio::test]
async fn reading_push_status_never_authenticates_creates_accounts_starts_or_writes_business() {
    let f=Fixture::new(vec![],false);let mut configuration=database_configuration(&f,true);
    configuration.runtime.as_mut().unwrap().profile.incoming=None;
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration}).unwrap();
    let report=f.manager.push_status().await.unwrap();assert!(report.initialized&&report.configuration.runtime_template_present);
    assert!(!report.configuration.sms_receiver_present&&!report.configuration.sip_request_receiver_selected);
    assert!(!report.configuration.call_receiver_present&&!report.configuration.call_control_present&&!report.configuration.call_media_present);
    assert!(report.warnings.contains(&"sms_receiver_missing_or_unsupported"));assert!(report.warnings.contains(&"call_media_missing"));
    assert!(report.native_accounts.is_empty());assert_eq!(report.stored_observations.app_sms_records,Some(0));
    assert!(f.script.paths().is_empty());assert!(f.manager.list().unwrap().is_empty());assert!(f.manager.app_accounts().unwrap().is_empty());
    assert_eq!(f.factory.build_count(),0);let text=serde_json::to_string(&report).unwrap();
    for private in ["fixture-","secret_id","salt","ClientId","SipPwd","phone_login_token","device_id"] {assert!(!text.contains(private));}
}

fn device_write(id:Option<&str>,revision:u64,account:Option<&str>,value:Option<&str>)->DeviceIdentityWriteRequest {
    DeviceIdentityWriteRequest {id:id.map(str::to_owned),expected_revision:revision,account_id:account.map(str::to_owned),
        device_id:value.map(|v|Secret::from_owned(v.to_owned()))}
}
fn device_fixture_configuration(f:&Fixture,enroll:bool)->DatabaseConfiguration {
    let mut configuration=database_configuration(f,false);
    configuration.http.device_id=Secret::from_owned(String::new());
    if enroll {configuration.http.endpoints.device_enroll=Some("enroll".to_owned());}
    configuration
}
fn enrolled_auth_steps()->Vec<Step> {
    let mut steps=vec![reply("/enroll",json!({"code":0,"body":{"enrollId":"fixture-enrolled-id"}}))];
    steps.extend(app_only_steps());steps
}
fn device_rows(f:&Fixture,table:&str)->i64 {
    f.temp.database.with_conn(|c|c.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0)).map_err(|_|"fixture_query_failed")).unwrap()
}
#[test]
fn device_mode_is_kept_in_the_private_configuration_roundtrip_without_changing_sdk_identity() {
    let f=Fixture::new(vec![],false);let mut configuration=database_configuration(&f,true);
    configuration.http.device_identity_mode=config::DeviceIdentityMode::Static;
    let restored=DatabaseConfiguration::decode_private(&configuration.encode_private().unwrap()).unwrap();
    assert_eq!(restored.http.device_identity_mode,config::DeviceIdentityMode::Static);
    assert_eq!(restored.runtime.unwrap().sdk.device_id.unwrap().expose_to_backend(),"fixture-original-device");
}
#[test]
fn device_login_alias_keys_are_realm_scoped_and_normalize_only_the_existing_login_identity() {
    let script=Arc::new(Script::new(vec![],false));let cfg=http_config(false);
    let client=Factory(script.clone()).build(&cfg).unwrap();let request=login_request(&id());
    let first=client.login_alias_key(&request).unwrap();
    let mut same=login_request(&id());same.username=Secret::from_owned("USER@example.invalid".into());
    assert_eq!(first,client.login_alias_key(&same).unwrap());
    let mut other=cfg;other.base_url=Secret::from_owned("https://other-realm.invalid/".into());
    assert_ne!(first,Factory(script).build(&other).unwrap().login_alias_key(&request).unwrap());
}
#[tokio::test]
async fn account_level_manual_device_update_is_reused_by_the_next_login_not_a_different_lookup_key() {
    let mut steps=enrolled_auth_steps();steps.extend(enrolled_auth_steps());
    let f=Fixture::new(steps,false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:device_fixture_configuration(&f,true)}).unwrap();
    let first=id();let mut request=app_only_request(&first);request.device_id=Some(Secret::from_owned("OWN-MANUAL-0001".into()));
    f.manager.submit_jegotrip(request).unwrap();let view=terminal(&f.manager,&first).await;assert_eq!(view.phase,"completed");assert_redacted(&view);
    let stored=f.manager.device_identities().unwrap().identities.remove(0);assert_eq!(stored.device_id,"OWN-MANUAL-0001");
    assert_eq!(stored.bound_account_id,view.account_id);assert_eq!(stored.masked_number.as_deref(),Some("***0123"));
    let updated=f.manager.set_device_identity(device_write(Some(&stored.id),stored.revision,view.account_id.as_deref(),Some("OWN-MANUAL-0002"))).unwrap();
    assert_eq!(updated.identities[0].revision,stored.revision+1);
    let second=id();f.manager.submit_jegotrip(app_only_request(&second)).unwrap();assert_eq!(terminal(&f.manager,&second).await.phase,"completed");
    let seen=lock(&f.script.seen);assert_eq!(seen[0].body["deviceId"],"OWN-MANUAL-0001");assert_eq!(seen[3].body["deviceId"],"OWN-MANUAL-0002");
    assert_eq!(f.manager.app_accounts().unwrap().len(),1);assert_eq!(f.factory.build_count(),0);
}
#[tokio::test]
async fn another_login_alias_reauthenticates_once_with_the_confirmed_numbers_existing_device() {
    let mut steps=enrolled_auth_steps();steps.extend(enrolled_auth_steps());steps.extend(enrolled_auth_steps());
    let f=Fixture::new(steps,false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:device_fixture_configuration(&f,true)}).unwrap();
    let first=id();f.manager.submit_jegotrip(app_only_request(&first)).unwrap();assert_eq!(terminal(&f.manager,&first).await.phase,"completed");
    let original=f.manager.device_identities().unwrap().identities.remove(0);
    let second=id();let mut request=app_only_request(&second);request.username=Secret::from_owned("second-alias@example.invalid".into());
    f.manager.submit_jegotrip(request).unwrap();let result=terminal(&f.manager,&second).await;assert_eq!(result.phase,"completed");assert_redacted(&result);
    let seen=lock(&f.script.seen);assert_ne!(seen[3].body["deviceId"],original.device_id);assert_eq!(seen[6].body["deviceId"],original.device_id);
    drop(seen);
    assert_eq!(f.script.paths(),["/enroll","/login","/user","/enroll","/login","/user","/enroll","/login","/user"]);
    assert_eq!(f.manager.app_accounts().unwrap().len(),1);assert_eq!(f.manager.device_identities().unwrap().identities.len(),1);
    assert_eq!(device_rows(&f,"jegotrip_device_aliases"),2);assert_eq!(f.factory.build_count(),0);
}
#[tokio::test]
async fn a_changed_server_number_does_not_rebind_a_used_device_or_reach_credentials_and_business() {
    let mut steps=app_only_steps();let mut changed=app_only_steps();
    changed[1]=reply("/user",json!({"code":0,"body":{"mobile":"7700900999","country_code":"44"}}));steps.extend(changed);
    let f=Fixture::new(steps,false);f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:device_fixture_configuration(&f,false)}).unwrap();
    let first=id();f.manager.submit_jegotrip(app_only_request(&first)).unwrap();let old=terminal(&f.manager,&first).await;assert_eq!(old.phase,"completed");
    let stored=f.manager.device_identities().unwrap().identities.remove(0);
    let next=id();f.manager.submit_jegotrip(app_only_request(&next)).unwrap();let failed=terminal(&f.manager,&next).await;
    assert_eq!(failed.error,Some("app_login_identity_mismatch"));assert_eq!(failed.business_state,"not_requested");assert_redacted(&failed);
    assert_eq!(f.script.paths(),["/login","/user","/login","/user"]);let still=f.manager.device_identities().unwrap().identities.remove(0);
    assert_eq!(still.device_id,stored.device_id);assert_eq!(still.bound_account_id,old.account_id);assert_eq!(still.revision,stored.revision);
}
#[tokio::test]
async fn device_mutations_are_blocked_during_login_and_busy_admission_does_not_allocate_more_ids() {
    let f=Fixture::new(vec![Step::Park("/login".into())],false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:device_fixture_configuration(&f,false)}).unwrap();
    let original=id();f.manager.submit_jegotrip(app_only_request(&original)).unwrap();
    tokio::time::timeout(Duration::from_secs(3),async {while f.script.paths().is_empty(){tokio::task::yield_now().await;}}).await.unwrap();
    let row=f.manager.device_identities().unwrap().identities.remove(0);assert!(row.in_use);
    assert_eq!(f.manager.set_device_identity(device_write(Some(&row.id),row.revision,None,Some("OWN-CHANGED-0001"))).err().unwrap().code,"app_login_device_identity_in_use");
    assert_eq!(f.manager.delete_device_identity(&row.id,row.revision).err().unwrap().code,"app_login_device_identity_in_use");
    assert_eq!(f.manager.submit_jegotrip(app_only_request(&id())).unwrap_err().code,"app_login_busy");
    assert_eq!(device_rows(&f,"jegotrip_device_identities"),1);assert_eq!(device_rows(&f,"jegotrip_device_aliases"),1);
    f.manager.shutdown().await;assert_eq!(f.manager.get(&original).unwrap().error,Some("app_login_interrupted"));
    assert!(!f.manager.device_identities().unwrap().identities[0].in_use);
    assert_eq!(f.manager.set_device_identity(device_write(None,0,None,None)).err().unwrap().code,"app_login_shutting_down");
}
#[tokio::test]
async fn failed_durable_attempt_admission_rolls_back_device_alias_and_history_allocation() {
    let f=Fixture::new(vec![],false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:device_fixture_configuration(&f,false)}).unwrap();
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fixture_reject_device_attempt BEFORE INSERT ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;")
        .map_err(|_|"fixture_setup_failed")).unwrap();
    assert_eq!(f.manager.submit_jegotrip(app_only_request(&id())).unwrap_err().code,"app_login_storage_failed");
    for table in ["jegotrip_device_identities","jegotrip_device_aliases","jegotrip_device_identity_history"] {assert_eq!(device_rows(&f,table),0);}
    assert!(f.script.paths().is_empty());assert!(f.manager.list().unwrap().is_empty());
}
#[tokio::test]
async fn concurrent_aliases_cannot_borrow_the_same_canonical_device_before_the_original_attempt_finishes() {
    let mut steps=app_only_steps();steps.push(Step::Park("/csb/token".into()));steps.extend(app_only_steps());
    let f=Fixture::new(steps,false);let mut configuration=database_configuration(&f,true);configuration.http.device_id=Secret::from_owned(String::new());
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration}).unwrap();
    let first=id();let mut request=app_only_request(&first);request.intent=LoginIntent::AttachHosting;request.start_receiver=false;
    f.manager.submit_jegotrip(request).unwrap();
    tokio::time::timeout(Duration::from_secs(3),async {while !f.script.paths().contains(&"/csb/token".to_owned()){tokio::task::yield_now().await;}}).await.unwrap();
    let next=id();let mut request=app_only_request(&next);request.username=Secret::from_owned("other-alias@example.invalid".into());
    f.manager.submit_jegotrip(request).unwrap();let result=terminal(&f.manager,&next).await;
    assert_eq!(result.error,Some("app_login_busy"));assert_eq!(result.business_state,"not_requested");assert_redacted(&result);
    assert_eq!(f.script.paths(),["/login","/user","/csb/token","/login","/user"]);
    assert_eq!(f.manager.app_accounts().unwrap().len(),1);f.manager.shutdown().await;
}

fn trust_check_configuration(f:&Fixture)->DatabaseConfiguration {
    let mut configuration=database_configuration(f,false);
    configuration.http.endpoints.device_enroll=Some("enroll".into());
    configuration.http.endpoints.security_verify=Some("security".into());
    configuration.http.endpoints.verification_trustful_device=Some("trust".into());
    configuration
}
fn trust_check_request(attempt:&str)->JegotripLoginRequest {
    let mut request=app_only_request(attempt);request.intent=LoginIntent::CheckDeviceTrust;
    request.device_id=Some(Secret::from_owned("D5C0199B-1644-4D33-8912-B8BD5AB49224".into()));request
}
fn trust_check_steps(code:i64,enabled:bool)->Vec<Step> {
    let mut steps=app_only_steps();
    steps.push(reply("/security",json!({"code":0,"body":{"authentication_code":code}})));
    steps.push(reply("/trust",json!({"code":0,"body":{"authentication_code":code}})));
    steps.push(reply("/state",json!({"code":0,"body":{"cs_forward":if enabled {"2"}else{"1"}}})));
    steps
}
#[tokio::test]
async fn phone_uuid_trust_check_uses_the_owned_identity_without_enrollment_phone_credentials_business_or_native_start() {
    let f=Fixture::new(trust_check_steps(1,true),true);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();
    let view=terminal(&f.manager,&attempt).await;assert_eq!(view.phase,"completed");assert_eq!(view.intent,LoginIntent::CheckDeviceTrust);
    assert_eq!(view.business_state,"not_requested");assert!(view.line_id.is_none());assert_redacted(&view);
    let observation=view.device_trust.as_ref().unwrap();
    assert_eq!(observation.security_verify.authentication_code,Some(1));assert_eq!(observation.existing_device.authentication_code,Some(1));
    assert_eq!(observation.hosting_enabled,Some(true));
    let identity=f.manager.device_identities().unwrap().identities.remove(0);
    assert_eq!(observation.device_identity_id,identity.id);assert_eq!(observation.device_identity_revision,identity.revision);
    assert_eq!(identity.bound_account_id,view.account_id);assert_eq!(identity.device_id,"D5C0199B-1644-4D33-8912-B8BD5AB49224");
    assert_eq!(f.script.paths(),["/login","/user","/security","/trust","/state"]);
    let seen=lock(&f.script.seen);
    assert!(seen[0].body.get("enrollId").is_none());
    assert_eq!(seen[2].body,json!({"deviceId":identity.device_id}));
    assert_eq!(seen[3].body,json!({"sso_token":"","equipment_type":"1","device_id":identity.device_id}));
    assert_eq!(seen[3].wire,seen[3].body,"existing-device query remains ordinary JSON");
    assert_eq!(seen[4].body,json!({"device_id":identity.device_id}));
    for value in seen.iter().skip(1) {assert_eq!(value.query["token"],"fixture-app-session");}
    drop(seen);
    assert_eq!(f.factory.build_count(),0);assert!(f.accounts.list(now() as u64).await.unwrap().is_empty());
    f.manager.shutdown().await;
}
#[tokio::test]
async fn trust_check_keeps_transport_failure_untrusted_and_unknown_business_as_separate_observations() {
    let mut steps=app_only_steps();steps.push(Step::Fail("/security".into()));
    steps.push(reply("/trust",json!({"code":0,"body":{"authentication_code":2,"token":"must-not-export"}})));
    steps.push(Step::Fail("/state".into()));let f=Fixture::new(steps,false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let attempt=id();f.manager.submit_jegotrip(trust_check_request(&attempt)).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.phase,"completed","query task completion is not successful authorization");
    let observation=view.device_trust.as_ref().unwrap();
    assert_eq!(observation.security_verify.state,device_trust::TrustQueryState::Failed);
    assert_eq!(observation.security_verify.error.as_deref(),Some("app_login_transport_failed"));
    assert_eq!(observation.existing_device.authentication_code,Some(2));assert_eq!(observation.hosting_enabled,None);
    assert_eq!(observation.business_query_error.as_deref(),Some("app_login_transport_failed"));
    assert_eq!(view.business_state,"not_requested");assert!(view.line_id.is_none());assert_eq!(f.factory.build_count(),0);
    let encoded=serde_json::to_string(&view).unwrap();assert!(!encoded.contains("must-not-export"));assert_redacted(&view);
    assert_eq!(f.script.paths(),["/login","/user","/security","/trust","/state"]);f.manager.shutdown().await;
}
#[test]
fn trust_check_requires_consent_no_receiver_and_rejects_forged_trust_and_hardware_inputs() {
    let f=Fixture::new(vec![],false);let mut request=trust_check_request(&id());request.consent=false;
    assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code,"app_login_request_invalid");
    let mut request=trust_check_request(&id());request.start_receiver=true;
    assert_eq!(f.manager.submit_jegotrip(request).unwrap_err().code,"app_login_request_invalid");
    for field in ["device_trust","trust_granted","hardware_imei","sso_token","phone_login_token"] {
        let mut value=json!({"attempt_id":id(),"username":"user@example.invalid","password":" p@ss ","consent":true,
            "intent":"check_device_trust","start_receiver":false});value[field]=json!("must-not-import");
        assert!(serde_json::from_value::<JegotripLoginRequest>(value).is_err());
    }
    assert_eq!(LoginIntent::default(),LoginIntent::EnableHosting);assert!(f.script.paths().is_empty());
}
#[test]
fn trust_observation_storage_is_bounded_owner_guarded_and_never_exports_reflected_fields() {
    let conn=rusqlite::Connection::open_in_memory().unwrap();store::initialize(&conn,"owner",10).unwrap();let attempt=id();
    store::insert_with_intent(&conn,&attempt,"owner",LoginIntent::CheckDeviceTrust,10).unwrap();
    let snapshot=device_identity::Snapshot {id:id(),device_id:Secret::from_owned("PRIVATE-UUID-0001".into()),revision:1};
    let observation=DeviceTrustObservation::capture(&snapshot,11,Ok(Some(http::TrustDecision {authentication_code:Some(1)})),Ok(None),Ok(false));
    let encoded=observation.encode().unwrap();assert!(!encoded.contains("PRIVATE-UUID-0001"));
    assert!(store::set_device_trust(&conn,&attempt,"another-owner",&observation,11).is_err());
    store::set_device_trust(&conn,&attempt,"owner",&observation,11).unwrap();
    assert_eq!(store::get(&conn,&attempt).unwrap().unwrap().device_trust.unwrap().device_identity_id,snapshot.id);
    let mut reflected:Value=serde_json::from_str(&encoded).unwrap();reflected["token"]=json!("secret");
    assert!(DeviceTrustObservation::decode(&reflected.to_string()).is_err());
    assert!(DeviceTrustObservation::decode(&"x".repeat(device_trust::MAX_OBSERVATION_BYTES+1)).is_err());
    let mut invalid=observation.clone();invalid.existing_device.authentication_code=Some(1);assert!(invalid.encode().is_err());
    store::finish(&conn,&attempt,"owner",None,12).unwrap();assert!(store::set_device_trust(&conn,&attempt,"owner",&observation,13).is_err());
    conn.execute("UPDATE jegotrip_login_attempts SET device_trust=?1 WHERE attempt_id=?2",rusqlite::params!["x".repeat(2049),attempt]).unwrap();
    assert!(store::get(&conn,&attempt).is_err());
}
#[tokio::test]
async fn trust_observation_persistence_failure_prevents_a_later_business_write_or_native_creation() {
    let f=Fixture::new(normal_steps(false),false);
    f.temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fixture_reject_trust BEFORE UPDATE OF device_trust ON jegotrip_login_attempts BEGIN SELECT RAISE(ABORT,'fixture'); END;")
        .map_err(|_|"fixture")).unwrap();
    let request=login_request(&id());let attempt=request.attempt_id.clone();f.manager.submit(request).unwrap();let view=terminal(&f.manager,&attempt).await;
    assert_eq!(view.error,Some("app_login_storage_failed"));assert_eq!(view.business_state,"not_requested");assert!(view.line_id.is_none());
    assert_eq!(f.script.paths(),["/login","/user","/csb/token","/voip","/state"]);assert_eq!(f.factory.build_count(),0);f.manager.shutdown().await;
}
#[tokio::test]
async fn a_phone_trust_observation_stays_historical_after_uuid_rotation_and_never_restores_a_receiver() {
    let mut steps=trust_check_steps(1,true);steps.extend(trust_check_steps(2,false));let f=Fixture::new(steps,false);
    f.manager.configure(ConfigurationWriteRequest {expected_revision:0,configuration:trust_check_configuration(&f)}).unwrap();
    let first=id();f.manager.submit_jegotrip(trust_check_request(&first)).unwrap();let original=terminal(&f.manager,&first).await;
    tokio::time::timeout(Duration::from_secs(4),async {while f.manager.active.available_permits()!=2 {tokio::task::yield_now().await;}}).await.unwrap();
    let identity=f.manager.device_identities().unwrap().identities.remove(0);
    f.manager.set_device_identity(device_write(Some(&identity.id),identity.revision,original.account_id.as_deref(),Some("F1C0199B-1644-4D33-8912-B8BD5AB49224"))).unwrap();
    let second=id();let mut request=trust_check_request(&second);request.device_id=None;f.manager.submit_jegotrip(request).unwrap();
    let next=terminal(&f.manager,&second).await;assert_eq!(original.account_id,next.account_id);
    assert_eq!(original.device_trust.as_ref().unwrap().security_verify.authentication_code,Some(1));
    assert_eq!(next.device_trust.as_ref().unwrap().security_verify.authentication_code,Some(2));
    assert!(next.device_trust.as_ref().unwrap().device_identity_revision>original.device_trust.as_ref().unwrap().device_identity_revision);
    assert_eq!(f.manager.get(&first).unwrap().device_trust.unwrap().security_verify.authentication_code,Some(1));
    let diagnostics=f.manager.push_status().await.unwrap();assert!(diagnostics.native_accounts.is_empty());
    assert_eq!(f.factory.build_count(),0);assert!(next.line_id.is_none());f.manager.shutdown().await;
}
