// Actions-only authenticated HTTP directory policy/shape/no-start regression.
// Directory counts/configuration are not current SDK, relationship or WRITE grants.
fn native_directory_request(revision:u64)->serde_json::Value{serde_json::json!({"expected_revision":revision,"operation":"relations","since":"-1","consent":true})}
#[tokio::test]
async fn native_directory_http_is_authenticated_strict_no_store_and_never_echoes_caller_credentials(){
 let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/native-directory/read";
 let(status,_,body)=account_request(&served,reqwest::Method::POST,path,Some(native_directory_request(1)),None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
 let cookie=authenticate(&served).await;
 for key in ["uid","token","reply","prefixes","endpoint","scope_epoch","confirmed","write_consent","authority"]{let mut value=native_directory_request(1);value[key]=serde_json::json!("fixture-must-not-echo");let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(value),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
 assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());assert!(state.app.app_logins.list().unwrap().is_empty());
}
#[tokio::test]
async fn native_directory_http_requires_canonical_signed64_and_opaque_group_not_phone_box_uri_or_boolean_permission(){
 let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/native-directory/read";
 let mut values=Vec::new();for since in ["", "-2", "01", "+1", "-0", "9223372036854775808", "-9223372036854775809"]{let mut v=native_directory_request(1);v["since"]=serde_json::json!(since);values.push(v);}
 for since in [serde_json::json!(1),serde_json::json!(null),serde_json::json!(true),serde_json::json!("1".repeat(21))]{let mut v=native_directory_request(1);v["since"]=since;values.push(v);}
 for id in ["", "15900000000", "1_2", "Org/1-2", "P2P/1_2", "[phone:+1000]", "Group:tcp -h fixture.invalid -p 1;", "0-2", "01-2", "1-0", "1-2\0"]{let mut v=native_directory_request(1);v["group_id"]=serde_json::json!(id);values.push(v);}
 let mut v=native_directory_request(1);v["group_id"]=serde_json::json!("1".repeat(509));values.push(v);
 for operation in ["send", "set_received", "mark_read", "ready", "", "Relations"]{let mut v=native_directory_request(1);v["operation"]=serde_json::json!(operation);values.push(v);}
 for extra in [serde_json::json!({"operation":"properties"}),serde_json::json!({"operation":"properties","group_id":"1-2","since":"0"}),serde_json::json!({"consent":1})]{let mut v=native_directory_request(1);for(key,value)in extra.as_object().unwrap(){v[key]=value.clone();}values.push(v);}
 for value in values{let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(value),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
 assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]
async fn native_directory_http_stored_opt_in_and_full_signed64_shapes_do_not_start_sdk_or_grant_current_read(){
 let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");
 for enabled in [false,true]{let mut config=im_runtime_config_json(&cert.root);if enabled{config["profile"]["native_receivers"]["im_messages"]["directory_queries"]=serde_json::json!(true);}
 let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);no_store(&headers);no_secrets(&body);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}/native-directory/read");
 for(revision,consent,wanted)in[(1,false,StatusCode::FORBIDDEN),(2,true,StatusCode::CONFLICT),(1,true,StatusCode::SERVICE_UNAVAILABLE)]{let mut v=native_directory_request(revision);v["consent"]=serde_json::json!(consent);let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(v),Some(&cookie)).await;assert_eq!(status,wanted);no_store(&headers);no_secrets(&body);}
 for v in [serde_json::json!({"expected_revision":1,"operation":"relations","group_id":null,"since":"9223372036854775807","consent":true}),serde_json::json!({"expected_revision":1,"operation":"relations","group_id":"1+2","since":"0","consent":true}),serde_json::json!({"expected_revision":1,"operation":"properties","group_id":"1-2","since":"-1","consent":true})]{let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(v),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE,"valid DATA shape is not current native admission");no_store(&headers);no_secrets(&body);}
 let view=state.app.app_accounts.get(&line,100).await.unwrap();assert_eq!(view.phase,"stopped");assert!(view.directory_queries_supported.is_none());assert!(!view.io_cleanup_required&&!view.incoming_message_ready&&!view.incoming_call_ready&&!view.outbound_authorized);assert!(state.app.app_accounts.im_messages(&line,128).unwrap().is_empty());assert!(state.app.app_accounts.im_send_history(&line,128).unwrap().is_empty());}
 assert!(state.app.app_logins.list().unwrap().is_empty());let rows:i64=state.app.database.with_conn(|c|c.query_row("SELECT (SELECT COUNT(*) FROM app_voice_runtime_leases)+(SELECT COUNT(*) FROM app_voice_im_receive_cursors)+(SELECT COUNT(*) FROM app_voice_im_read_receipts)+(SELECT COUNT(*) FROM app_voice_im_user_reads)+(SELECT COUNT(*) FROM app_voice_im_read_status_actions)+(SELECT COUNT(*) FROM app_voice_im_outbox)",[],|r|r.get(0)).map_err(|_|"fixture directory no grant")).unwrap();assert_eq!(rows,0);
}
