// Actual Actions HTTP/auth/DTO boundary; fixture hold is within original connector deadline.
async fn connect_detail_http_timeout(step:crate::connectivity::app_voice::providers::jegotrip::startup_diagnostics::ConnectFailureStep){
    use crate::connectivity::app_voice::providers::jegotrip as n;
    let mut state=build_test_router().await.expect("Actions private bus required; no skip");let factory=n::connect_phase_test_support::ConnectDetailFactory::for_step(step);
    state.app.app_accounts=std::sync::Arc::new(n::account_runtime::AccountManager::with_factory(state.app.database.clone(),factory.clone()));
    let router=build_router(state.app.clone(),CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any));let served=serve(router).await;
    let(status,_,body)=account_request(&served,reqwest::Method::GET,"/api/non-3gpp/accounts",None,None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
    let cookie=authenticate(&served).await;let cert=Certificates::generate("IP:127.0.0.1");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let mut cfg=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",listener.local_addr().unwrap().port());cfg.credentials.sdk.expires_at_unix_seconds=None;let config=serde_json::from_slice::<serde_json::Value>(&cfg.encode_private().unwrap()).unwrap()["config"].clone();
    let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let base=format!("/api/non-3gpp/accounts/{line}");
    let(status,_,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);let before:serde_json::Value=serde_json::from_str(&body).unwrap();assert!(before["data"].get("startup_connect_detail").is_none());assert_eq!(factory.builds(),0);
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);
    let(status,headers,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let failed:serde_json::Value=serde_json::from_str(&body).unwrap();let data=&failed["data"];
    assert_eq!(data["startup_failure"].as_object().unwrap().len(),3);assert_eq!(data["startup_failure"]["stage"],"router_connect");assert_eq!(data["startup_failure"]["code"],"jegotrip_connector_timeout");assert_eq!(data["startup_connect_detail"].as_object().unwrap().len(),2);assert_eq!(data["startup_connect_detail"]["phase"],"account");assert_eq!(data["startup_connect_detail"]["step"],serde_json::to_value(step).unwrap());assert!(data.get("startup_tls_detail").is_none());
    assert_eq!(data["incoming_message_ready"],false);assert_eq!(data["incoming_call_ready"],false);assert_eq!(data["outbound_authorized"],false);assert_eq!(data["phase"],"disconnected");assert_eq!(factory.builds(),1);
    for secret in ["fixture","127.0.0.1","root_der","certificate","hostname","phone_login_token","private_config"]{assert!(!body.contains(secret));}
    assert!(tokio::time::timeout(std::time::Duration::from_millis(20),listener.accept()).await.is_err());
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
    let(_,_,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;let stopped:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(stopped["data"]["phase"],"stopped");assert_eq!(stopped["data"]["io_cleanup_required"],false);assert!(stopped["data"].get("startup_connect_detail").is_none());
    assert_eq!(state.app.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases",[],|r|r.get::<_,i64>(0)).map_err(|_|"test_query_failed")).unwrap(),0);factory.gate().release();
}
#[tokio::test]
async fn startup_connect_dns_timeout_http_is_authenticated_finite_no_store_data_and_stop_joins(){connect_detail_http_timeout(crate::connectivity::app_voice::providers::jegotrip::startup_diagnostics::ConnectFailureStep::Dns).await;}
#[tokio::test]
async fn startup_connect_tcp_timeout_http_keeps_legacy_failure_and_no_tls_or_ready_claim(){connect_detail_http_timeout(crate::connectivity::app_voice::providers::jegotrip::startup_diagnostics::ConnectFailureStep::Tcp).await;}
