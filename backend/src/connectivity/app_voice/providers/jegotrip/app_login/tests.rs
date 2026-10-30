//! Actions-only tests: synthetic App credentials, SQLite, real loopback HTTPS
//! and the existing native TLS/Common peer. Never an APK or real service run.
use super::*;
use super::{config::HttpDeployment, crypto::JsonDoc, http::{Transport, Exchange, WireRequest, WireResponse}};
use super::super::{account_test_support::{TempDatabase, QueuedFactory, config_json, peer}, account_store,
    tls_engine::tests::Certificates, rpc_envelope::decode_request, runtime_backend::tests::next_non_poll};
use serde_json::{json, Value};
use std::collections::{VecDeque, BTreeMap};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use zeroize::Zeroizing;

const SIP_CIPHER:&str="hjhmeor4ER9SUekKGe7UZQ==";
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
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_login_finish BEFORE UPDATE OF completed ON app_voice_login_attempts WHEN NEW.completed=1 BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(|_|"fixture")).unwrap();
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
