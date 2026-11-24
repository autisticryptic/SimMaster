// Included in the existing Actions/private-bus real authenticated HTTP module.
fn relation_http_body()->serde_json::Value{serde_json::json!({"expected_revision":1,"group_id":null,"nonce":"a".repeat(32),"additions":[{"label":"private-http-label","relation_type":258,"first":"private-http-note"}],"updates":[],"removals":[{"label":"private-http-removal"}],"consent":true,"write_consent":true})}
fn relation_http_config(root:&str)->serde_json::Value{let mut cfg=im_runtime_config_json(root);cfg["profile"]["relation_updates"]=serde_json::json!({"additions":true,"updates":true,"removals":true});cfg}
#[tokio::test]async fn relation_http_both_routes_require_existing_auth_without_creating_sdk_or_intent(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/relation-writes";
    for method in [reqwest::Method::GET,reqwest::Method::POST]{let(status,_,body)=account_request(&served,method,path,Some(relation_http_body()),None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);assert!(!body.contains("private-http"));}
    assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());assert!(state.app.app_logins.list().unwrap().is_empty());
}
#[tokio::test]async fn relation_http_strict_dto_cannot_supply_uid_token_owner_route_result_or_nested_authority(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/relation-writes";
    for key in ["uid","current_uid","token","owner","scope_epoch","selected_session_id","endpoint","route","callback","result","base_time","update_time","ready","grant"]{
        let mut input=relation_http_body();input[key]=serde_json::json!("fixture-must-not-echo");let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
    for extra in [serde_json::json!({"label":"private-http-label","relation_type":258,"token":"fixture-must-not-echo"}),serde_json::json!({"label":"private-http-label","relation_type":258,"properties":[{"key":[],"value":[],"scope":"fixture-must-not-echo"}]})]{
        let mut input=relation_http_body();input["additions"]=serde_json::json!([extra]);let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
}
#[tokio::test]async fn relation_http_native_type_nonce_group_and_cross_map_bounds_fail_before_projection_copy(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let path="/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/relation-writes";
    let mut bad=Vec::new();for ty in [0,1,17,20,256,260,4096,u32::MAX]{let mut v=relation_http_body();v["additions"][0]["relation_type"]=serde_json::json!(ty);bad.push(v);}
    for nonce in ["".to_owned(),"A".repeat(32),"a".repeat(31),"a".repeat(33)]{let mut v=relation_http_body();v["nonce"]=serde_json::json!(nonce);bad.push(v);}
    for group in ["","1_2","P2P/1_2","Org/1-2","01-2","1--2","#Group:tcp -h fixture.invalid -p 1;"]{let mut v=relation_http_body();v["group_id"]=serde_json::json!(group);bad.push(v);}
    let mut v=relation_http_body();v["updates"]=v["additions"].clone();bad.push(v);
    let mut v=relation_http_body();v["removals"][0]["label"]=v["additions"][0]["label"].clone();bad.push(v);
    let mut v=relation_http_body();v["additions"]=serde_json::json!([]);v["removals"]=serde_json::json!([]);bad.push(v);
    let mut v=relation_http_body();v["additions"]=serde_json::json!(vec![serde_json::json!({"label":"x","relation_type":258});129]);bad.push(v);
    let mut v=relation_http_body();v["additions"][0]["first"]=serde_json::json!("x".repeat(65537));bad.push(v);
    let mut v=relation_http_body();v["additions"][0]["properties"]=serde_json::json!([{"key":[1],"value":[]},{"key":[1],"value":[2]}]);bad.push(v);
    for input in bad{let(status,headers,body)=account_request(&served,reqwest::Method::POST,path,Some(input),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);assert!(!body.contains("private-http"));}
    assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
}
#[tokio::test]async fn relation_http_installation_two_consents_revision_and_current_sdk_are_distinct(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");
    let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":relation_http_config(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}/relation-writes");
    for(revision,consent,write,wanted)in [(1,false,true,StatusCode::FORBIDDEN),(1,true,false,StatusCode::FORBIDDEN),(2,true,true,StatusCode::CONFLICT),(1,true,true,StatusCode::SERVICE_UNAVAILABLE)]{
        let mut input=relation_http_body();input["expected_revision"]=serde_json::json!(revision);input["consent"]=serde_json::json!(consent);input["write_consent"]=serde_json::json!(write);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(input),Some(&cookie)).await;assert_eq!(status,wanted);no_store(&headers);no_secrets(&body);for hidden in ["private-http",&"a".repeat(32),"nonce_tag","request_id","ready","token"]{assert!(!body.contains(hidden));}
    }
    let view=state.app.app_accounts.get(&line,100).await.unwrap();assert_eq!(view.phase,"stopped");assert_eq!(view.relation_updates_supported,None);assert!(!view.outbound_authorized&&!view.incoming_message_ready&&!view.incoming_call_ready);assert!(!view.io_cleanup_required);
    let count:i64=state.app.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_relation_writes WHERE line_id=?1",[&line],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(count,0);
}
#[tokio::test]async fn relation_http_configuration_needs_native_common_clientex_im_but_not_read_or_send_policy(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let good=relation_http_config(&cert.root);
    for mutation in [serde_json::json!({"client_ex":false}),serde_json::json!({"native_receivers":{}}),serde_json::json!({"relation_updates":{"additions":false,"updates":false,"removals":false}}),serde_json::json!({"relation_updates":{"additions":true,"updates":true,"removals":true,"grant":true}})]{
        let mut cfg=good.clone();for(key,value)in mutation.as_object().unwrap(){cfg["profile"][key]=value.clone();}
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":cfg})),Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":good})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);no_store(&headers);no_secrets(&body);
}
#[tokio::test]async fn relation_http_history_queries_are_bounded_private_read_only_and_never_request_sdk_start(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");
    let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":relation_http_config(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}/relation-writes");
    for suffix in ["?limit=0","?limit=129","?limit=1&token=fixture-must-not-echo","?limit=1&consent=true"]{let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}{suffix}"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);}
    let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}?limit=128"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"],serde_json::json!([]));assert_eq!(state.app.app_accounts.get(&line,100).await.unwrap().phase,"stopped");
}
#[tokio::test]async fn relation_http_saved_result_exports_only_finite_counts_exact_signed_times_and_nonce_tag(){
    let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");
    let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":relation_http_config(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);
    state.app.database.with_conn(|c|c.execute("INSERT INTO app_voice_relation_writes(line_id,revision,user_nonce,intent_hash,identity_key,run_nonce,scope_epoch,target_kind,additions,updates,removals,state,request_id,base_time,update_time,updated_at) VALUES(?1,1,?2,zeroblob(32),zeroblob(32),?3,'private-fixture-epoch','buddy',1,0,1,1,2,?4,?5,'2026-01-01T00:00:00Z')",rusqlite::params![line,"a".repeat(32),"b".repeat(32),i64::MIN,i64::MAX]).map_err(|_|"fixture stored DATA")).unwrap();
    let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("/api/non-3gpp/accounts/{line}/relation-writes?limit=128"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let json:serde_json::Value=serde_json::from_str(&body).unwrap();let v=&json["data"][0];assert_eq!(v["state"],"confirmed");assert_eq!(v["base_time"],"-9223372036854775808");assert_eq!(v["update_time"],"9223372036854775807");assert_eq!(v["nonce_tag"].as_str().unwrap().len(),64);
    for key in ["request_id","user_nonce","run_nonce","scope_epoch","identity_key","intent_hash","arguments_hash","token","ready",&"a".repeat(32)]{assert!(!body.contains(key));}assert_eq!(state.app.app_accounts.get(&line,100).await.unwrap().phase,"stopped");
}
#[tokio::test]async fn relation_http_authenticated_original_peer_write_commits_before_response_and_no_history_replay(){
    Box::pin(async{
        use crate::connectivity::app_voice::providers::jegotrip as n;
        let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");
        let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&tokio_rustls::rustls::version::TLS13);let mut cfg=relation_http_config(&cert.root);
        cfg["profile"]["route"]["router_common"]["reference"]=serde_json::json!(format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"));cfg["profile"]["route"]["router_common"]["anchors"][0]["port"]=serde_json::json!(port);
        let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":cfg})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let(finish,finishing)=tokio::sync::oneshot::channel();
        let peer=tokio::spawn(Box::pin(n::relation_http_test_peer::serve(listener,server,state.app.database.clone(),line.clone(),finishing)));
        // The original start endpoint reports submission as 202, not Hosted.
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("/api/non-3gpp/accounts/{line}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::ACCEPTED);no_store(&headers);no_secrets(&body);
        tokio::time::timeout(std::time::Duration::from_secs(6),async{while state.app.app_accounts.get(&line,100).await.unwrap().phase!="hosted"{tokio::task::yield_now().await;}}).await.unwrap();
        let path=format!("/api/non-3gpp/accounts/{line}/relation-writes");let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(relation_http_body()),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let json:serde_json::Value=serde_json::from_str(&body).unwrap();let data=&json["data"];assert_eq!(data["state"],"confirmed");assert_eq!(data["base_time"],"-9223372036854775808");assert_eq!(data["update_time"],"9223372036854775807");assert_eq!(data["removals"],1);
        let events=state.app.database.get_recent_app_events(Some(&line),32).unwrap();assert_eq!(events.iter().filter(|e|e.event_type=="app_voice.relations_updated").count(),1,"transaction commits before protected HTTP result");
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&path,Some(relation_http_body()),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);let replay:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(&replay["data"],data);
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}?limit=128"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert_eq!(&serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"][0],data);
        finish.send(()).unwrap();let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("/api/non-3gpp/accounts/{line}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);peer.await.unwrap();assert!(!state.app.app_accounts.get(&line,100).await.unwrap().io_cleanup_required);
    }).await;
}
