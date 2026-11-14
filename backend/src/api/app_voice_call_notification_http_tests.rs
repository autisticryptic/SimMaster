// Actions-only real HTTP authentication/metadata; no SDK or channel starts.
#[tokio::test]
async fn call_notification_metadata_route_requires_auth_and_invalid_query_never_mutates_work(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/call-notifications";
    let (status,_,body)=account_request(&served,reqwest::Method::GET,path,None,None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
    let cookie=authenticate(&served).await;
    for query in ["?limit=0","?limit=129","?limit=1&token=fixture-must-not-echo"]{
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}{query}"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
    assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]
async fn stored_account_call_notice_metadata_is_read_only_empty_and_never_claims_an_active_call(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("DNS:fixture.invalid");
    let (status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);
    let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("/api/non-3gpp/accounts/{line}/call-notifications?limit=16"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"],serde_json::json!([]));
    assert_eq!(state.app.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_call_notification_jobs",[],|r|r.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap(),0);
    assert_eq!(state.app.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM call_history",[],|r|r.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap(),0);
}
