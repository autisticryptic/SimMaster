// Actions-only authenticated HTTP admission/readback. No SDK socket or channel starts.
fn invite_config_json(root:&str)->serde_json::Value{
    use crate::connectivity::app_voice::providers::jegotrip as n;
    let mut config=n::account_config::tests::native_fixture(root,"127.0.0.1",1);config.credentials.sdk.expires_at_unix_seconds=None;
    config.profile.incoming=None;config.profile.client_ex=true;config.profile.native_call_control=true;
    config.profile.native_receivers=Some(n::native_receiver::NativeReceiverConfig{im_messages:None,direct_domains:None,invite_notifications:Some(n::native_invite_notifications::NativeInviteConfig{timeout_ms:1000})});
    config.call_media=n::account_call_test_support::call_config(root,root).call_media;
    serde_json::from_slice::<serde_json::Value>(&config.encode_private().unwrap()).unwrap()["config"].clone()
}
#[tokio::test]
async fn invite_configuration_http_requires_original_native_call_consumer_and_never_starts_sdk(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let base=invite_config_json(&cert.root);
    let mut cases=Vec::new();let mut media=base.clone();media["call_media"]=serde_json::Value::Null;cases.push(media);let mut client=base.clone();client["profile"]["client_ex"]=serde_json::json!(false);cases.push(client);let mut control=base.clone();control["profile"]["native_call_control"]=serde_json::json!(false);cases.push(control);let mut timeout=base.clone();timeout["profile"]["native_receivers"]["invite_notifications"]["timeout_ms"]=serde_json::json!(0);cases.push(timeout);
    for config in cases{let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
    assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":base})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);no_store(&headers);no_secrets(&body);
    let line=created_line(&body);let view=state.app.app_accounts.get(&line,100).await.unwrap();assert!(!view.incoming_call_ready);assert!(!view.incoming_message_ready);assert!(!view.outbound_authorized);assert!(!view.io_cleanup_required);
    assert_eq!(state.app.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_invite_notification_receipts",[],|row|row.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap(),0);
}
#[tokio::test]
async fn invite_call_status_http_is_authenticated_no_store_and_stored_configuration_is_not_consumption(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/calls";
    let(status,_,body)=account_request(&served,reqwest::Method::GET,path,None,None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
    let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":invite_config_json(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);
    let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("/api/non-3gpp/accounts/{line}/calls?token=fixture-must-not-echo"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert!(!body.contains("fixture-must-not-echo"));assert!(!body.contains("sdk_invite_notification"));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"],serde_json::json!([]),"existing authenticated call-list API returns an empty list without an active call/consumption");
    for table in ["call_history","app_voice_invite_notification_receipts"]{assert_eq!(state.app.database.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap(),0);}
}
