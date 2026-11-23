// Original Actions-only real HTTP/auth/DTO boundary. No Provider/phone SDK.
#[tokio::test]
async fn startup_tls_detail_http_is_authenticated_data_and_preserves_legacy_failure_and_stop(){
    use crate::connectivity::app_voice::providers::jegotrip as n;
    let state=build_test_router().await.expect("Actions private bus required; no skip");let served=serve(state.router.clone()).await;
    let(status,_,body)=account_request(&served,reqwest::Method::GET,"/api/non-3gpp/accounts",None,None).await;assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
    let cookie=authenticate(&served).await;let cert=Certificates::generate("DNS:wrong.fixture.invalid");let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server_config=cert.server_config(&tokio_rustls::rustls::version::TLS13);
    let server=tokio::spawn(async move{let(tcp,_)=listener.accept().await.unwrap();if let Ok(mut tls)=tokio_rustls::TlsAcceptor::from(server_config).accept(tcp).await{use tokio::io::AsyncReadExt;let mut buffer=[0;32];assert!(matches!(tls.read(&mut buffer).await,Ok(0)|Err(_)));}});
    let mut cfg=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",port);cfg.credentials.sdk.expires_at_unix_seconds=None;
    let config=serde_json::from_slice::<serde_json::Value>(&cfg.encode_private().unwrap()).unwrap()["config"].clone();
    let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let base=format!("/api/non-3gpp/accounts/{line}");
    let(status,_,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);let before:serde_json::Value=serde_json::from_str(&body).unwrap();assert!(before["data"].get("startup_tls_detail").is_none());
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);
    let(status,headers,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let failed:serde_json::Value=serde_json::from_str(&body).unwrap();
    let data=&failed["data"];assert_eq!(data["startup_failure"].as_object().unwrap().len(),3);assert_eq!(data["startup_failure"]["code"],"jegotrip_tls_handshake_failed");assert_eq!(data["startup_tls_detail"].as_object().unwrap().len(),2);assert_eq!(data["startup_tls_detail"]["phase"],"account");assert_eq!(data["startup_tls_detail"]["reason"],"name_mismatch");
    assert_eq!(data["incoming_message_ready"],false);assert_eq!(data["incoming_call_ready"],false);assert_eq!(data["outbound_authorized"],false);
    for secret in ["wrong.fixture","phone_login_token","root_der","certificate","private_config"]{assert!(!body.contains(secret));}
    let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{base}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
    let(_,_,body)=account_request(&served,reqwest::Method::GET,&base,None,Some(&cookie)).await;let stopped:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(stopped["data"]["phase"],"stopped");assert!(stopped["data"].get("startup_tls_detail").is_none());server.await.unwrap();
}
