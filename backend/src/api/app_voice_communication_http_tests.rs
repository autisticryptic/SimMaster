// Included in app_voice_accounts: real HTTP auth/CAS/no-store, Actions-only.
fn communication_http_nonce()->String{
    let mut bytes=[0u8;16];ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(),&mut bytes).unwrap();bytes.iter().map(|b|format!("{b:02x}")).collect()
}
fn communication_http_configuration()->serde_json::Value{
    serde_json::json!({"version":1,"runtime":null,"http":{"base_url":"https://fixture-app.invalid/","anonymous_token":"fixture-anonymous","signature_salt":"fixture-signature-salt",
        "password_salt":"fixture-password-salt","sip_password_key":"0123456789abcdefABCDEFGH","device_id":"fixture-device",
        "endpoints":{"login":"login","user_info":"user","phone_token":"csb/token","voip_status":"voip","call_status":"state","enable_hosting":"enable"},
        "communication":{"allow_business_writes":false,"profile":"profile","phone_service":"phone-service","display_status":"display","set_voip_profile":"set-voip","close_forward":"close","jego_m_status":"jego","logout":"logout"},
        "body_protection":null,"root_der_base64":null,"user_agent":null,"enroll_id":null}})
}
fn communication_http_input()->serde_json::Value{
    serde_json::json!({"attempt_id":"0123456789abcdef0123456789abcdef","username":"fixture-user","password":"fixture-password","country_code":null,"intent":"communication",
        "device_id":"D5C0199B-1644-4D33-8912-B8BD5AB49224","consent":true,"write_consent":false,"start_receiver":false,"expected_configuration_revision":1,"communication_operation":{"kind":"query_profile"}})
}
#[tokio::test]
async fn communication_api_requires_auth_and_rejects_unknown_or_unconsented_operations_without_private_echo(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/jegotrip/login/attempts";
    let (status,_,body)=account_request(&served,reqwest::Method::POST,path,Some(communication_http_input()),None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
    let cookie=authenticate(&served).await;
    for mutation in 0..6{
        let mut value=communication_http_input();match mutation{
            0=>value["communication_operation"]["url"]=serde_json::json!("https://fixture-private.invalid/"),
            1=>value["communication_operation"]["kind"]=serde_json::json!("fixture-invalid-operation"),
            2=>value["communication_operation"]=serde_json::json!({"kind":"set_phone_service","status":3}),
            3=>value["expected_configuration_revision"]=serde_json::Value::Null,
            4=>value["start_receiver"]=serde_json::json!(true),_=>value["device_id"]=serde_json::Value::Null,
        }
        let (status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(value),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST,"{mutation}");no_store(&headers);no_secrets(&body);assert!(body.contains("app_login_request_invalid"));
    }
    assert!(state.app.app_logins.list().unwrap().is_empty());assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]
async fn communication_api_configuration_is_same_origin_default_denied_and_revision_guarded_before_any_login(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let nonce=communication_http_nonce();
    let lock_path=std::env::temp_dir().join(format!("comm-api-{nonce}.lock"));state.app.app_accounts.claim_process_ownership(&lock_path).unwrap();state.app.app_logins.initialize(std::env::temp_dir().join(format!("missing-comm-{nonce}.json"))).unwrap();
    let served=serve(build_router(state.app.clone(),CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))).await;let cookie=authenticate(&served).await;
    for endpoint in ["https://other.invalid/profile","http://fixture.invalid/profile","profile?token=fixture-private"]{
        let mut cfg=communication_http_configuration();cfg["http"]["communication"]["profile"]=serde_json::json!(endpoint);
        let (status,headers,body)=account_request(&served,reqwest::Method::PUT,"/api/jegotrip/config",Some(serde_json::json!({"expected_revision":0,"configuration":cfg})),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
    let (status,headers,body)=account_request(&served,reqwest::Method::PUT,"/api/jegotrip/config",Some(serde_json::json!({"expected_revision":0,"configuration":communication_http_configuration()})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
    let (status,headers,body)=account_request(&served,reqwest::Method::GET,"/api/jegotrip/login/setup",None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let value:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(value["data"]["native_configured"],false);assert_eq!(value["data"]["communication_actions"],serde_json::json!(["query_profile","query_phone_service","query_display_status","query_forwarding_state"]));
    let mut wrong=communication_http_input();wrong["expected_configuration_revision"]=serde_json::json!(0);let (status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/jegotrip/login/attempts",Some(wrong),Some(&cookie)).await;assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);
    let mut write=communication_http_input();write["communication_operation"]=serde_json::json!({"kind":"logout"});write["write_consent"]=serde_json::json!(true);let (status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/jegotrip/login/attempts",Some(write),Some(&cookie)).await;assert_eq!(status,StatusCode::FORBIDDEN);no_store(&headers);no_secrets(&body);assert!(body.contains("app_login_communication_writes_disabled"));
    assert!(state.app.app_logins.list().unwrap().is_empty());assert!(state.app.app_logins.device_identities().unwrap().identities.is_empty());let _=std::fs::remove_file(lock_path);
}
#[tokio::test]
async fn communication_api_missing_endpoint_and_mixed_legacy_intent_never_create_an_identity_or_attempt(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let nonce=communication_http_nonce();let lock_path=std::env::temp_dir().join(format!("comm-missing-{nonce}.lock"));state.app.app_accounts.claim_process_ownership(&lock_path).unwrap();state.app.app_logins.initialize(std::env::temp_dir().join(format!("comm-missing-{nonce}.json"))).unwrap();
    let served=serve(build_router(state.app.clone(),CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))).await;let cookie=authenticate(&served).await;let mut cfg=communication_http_configuration();cfg["http"]["communication"]["profile"]=serde_json::Value::Null;
    let (status,_,_)=account_request(&served,reqwest::Method::PUT,"/api/jegotrip/config",Some(serde_json::json!({"expected_revision":0,"configuration":cfg})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);
    let (status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/jegotrip/login/attempts",Some(communication_http_input()),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);assert!(body.contains("app_login_communication_not_configured"));
    let mut value=communication_http_input();value["intent"]=serde_json::json!("authenticate");let (status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/jegotrip/login/attempts",Some(value),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    assert!(state.app.app_logins.list().unwrap().is_empty());assert!(state.app.app_logins.device_identities().unwrap().identities.is_empty());let _=std::fs::remove_file(lock_path);
}
