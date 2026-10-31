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
fn full_database_private_mirror_roundtrip_retains_runtime_without_personal_login_fields() {
    let f = Fixture::new(vec![], true);
    let configuration = database_configuration(&f, true);
    let bytes = configuration.encode_private().unwrap();
    let mut expected = deployment_json(&f.cert.root, true);
    for key in ["client_config_decode_key", "reference_decode_key"] { expected["runtime"]["sdk"][key] = Value::Null; }
    for key in ["call_incoming", "call_control"] { expected["runtime"]["profile"][key] = Value::Null; }
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
struct Recorded { path:String, query:BTreeMap<String,String>, body:Value }
struct Script { steps:Mutex<VecDeque<Step>>, seen:Mutex<Vec<Recorded>>, protection:Option<config::BodyProtection> }
impl Script {
    fn new(steps:Vec<Step>,encrypted:bool)->Self { Self {steps:Mutex::new(steps.into()),seen:Mutex::new(vec![]),protection:http_config(encrypted).body_protection} }
    fn paths(&self)->Vec<String> {lock(&self.seen).iter().map(|r|r.path.clone()).collect()}
}
impl Transport for Script {
    fn exchange(&self,request:WireRequest)->Exchange<'_> {Box::pin(async move {
        let path=request.url.path().to_owned();let encrypted=!matches!(path.as_str(),"/voip"|"/state") && self.protection.is_some();
        let payload=if request.body.is_empty() {Value::Null} else {
            let mut doc=crypto::parse(&request.body).unwrap();
            if encrypted {crypto::decrypt_body(&mut doc,self.protection.as_ref()).unwrap();doc.0["body"].clone()} else {doc.0.clone()}
        };
        lock(&self.seen).push(Recorded {path:path.clone(),query:request.url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect(),body:payload});
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
async fn wrong_https_root_refuses_before_any_login_body_is_delivered() {
    let cert=Certificates::generate("IP:127.0.0.1");let other=Certificates::generate("IP:127.0.0.1");
    let (port,task)=https_once(cert.server_config(&rustls::version::TLS13),String::new()).await;
    let mut cfg=http_config(false);cfg.base_url=Secret::from_owned(format!("https://127.0.0.1:{port}/"));cfg.root_der_base64=Some(Secret::from_owned(other.root.clone()));
    let mut client=LiveFactory.build(&cfg).unwrap();assert_eq!(client.login(&login_request(&id())).await.unwrap_err().code,"app_login_transport_failed");assert!(task.await.unwrap().is_none());
}
