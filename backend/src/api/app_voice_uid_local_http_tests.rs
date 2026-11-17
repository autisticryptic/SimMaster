// Actions-only HTTP guards for local UID DATA; not current auth or remote ACK.
#[tokio::test]
async fn uid_local_http_requires_original_current_account_and_does_not_accept_caller_local_report(){
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/user-entry/lookup";
    for key in ["report","local_uid","resolved_uid","ready","acknowledged"]{let mut input=serde_json::json!({"expected_revision":1,"recipient":"[uid:7_9]","consent":true});input[key]=serde_json::json!(true);let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);assert!(!body.contains("7_9"));}
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(serde_json::json!({"expected_revision":1,"recipient":"[uid:7_9]","consent":false})),Some(&cookie)).await;assert_eq!(status,StatusCode::FORBIDDEN);no_store(&headers);no_secrets(&body);assert!(!body.contains("7_9"));assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]
async fn uid_local_http_stopped_account_cannot_acquire_sdk_scope_from_valid_uid_syntax(){
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":im_runtime_config_json(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}/user-entry/lookup");
    for recipient in ["0_1","[uid:7_9]","[uid:7_9@foreign.invalid]"]{let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(serde_json::json!({"expected_revision":1,"recipient":recipient,"consent":true})),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);assert!(!body.contains("7_9"));}
    let view=state.app.app_accounts.get(&line,100).await.unwrap();assert!(view.user_entry_uid_local_supported.is_none());assert_eq!(view.phase,"stopped");assert!(!view.incoming_message_ready&&!view.incoming_call_ready&&!view.outbound_authorized&&!view.io_cleanup_required);
}
