// Actions-only HTTP authentication/input/privacy; no actual SDK/410 invocation.
#[tokio::test]
async fn im_reconciliation_http_requires_auth_and_rejects_caller_status_index_uid_nonce_or_token(){
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/im-user-reads/1/reconcile";
    let(status,_,body)=account_request(&served,reqwest::Method::POST,path,None,None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);let cookie=authenticate(&served).await;
    for field in ["nonce","index","uid","token","confirmed"]{let mut input=serde_json::json!({"expected_revision":1,"consent":true});input[field]=serde_json::json!("fixture-private-no-echo");let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);assert!(!body.contains("fixture-private-no-echo"));}
    assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]
async fn im_reconciliation_http_needs_separate_read_consent_and_current_account_before_any_io(){
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":im_runtime_config_json(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}/im-user-reads/1/reconcile");
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(serde_json::json!({"expected_revision":1,"consent":false})),Some(&cookie)).await;assert_eq!(status,StatusCode::FORBIDDEN);no_store(&headers);no_secrets(&body);
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(serde_json::json!({"expected_revision":2,"consent":true})),Some(&cookie)).await;assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(serde_json::json!({"expected_revision":1,"consent":true})),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);
    let view=state.app.app_accounts.get(&line,100).await.unwrap();assert_eq!(view.phase,"stopped");assert!(!view.incoming_message_ready&&!view.incoming_call_ready&&!view.outbound_authorized&&!view.io_cleanup_required);assert!(view.im_reconciliation_supported.is_none());
}
#[tokio::test]
async fn im_reconciliation_http_canonical_local_record_id_and_strict_body_cannot_forge_original_context(){
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let base="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/im-user-reads";
    for id in ["0","-1","01","9223372036854775808"]{let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/{id}/reconcile"),Some(serde_json::json!({"expected_revision":1,"consent":true})),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
    for input in [serde_json::json!({"expected_revision":1}),serde_json::json!({"expected_revision":1,"consent":"true"}),serde_json::json!({"expected_revision":1,"consent":true,"status":{"P:P_UDMR-N":"7"}})]{let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/1/reconcile"),Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
}
