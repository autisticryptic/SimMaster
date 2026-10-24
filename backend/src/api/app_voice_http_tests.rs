// Included inside main's Actions-only real HTTP/router test module.
mod app_voice_accounts {
    use super::*;
    use crate::connectivity::app_voice::providers::jegotrip::{account_store, account_test_support::config_json,
        tls_engine::tests::Certificates};

    async fn account_request(served: &Served, method: reqwest::Method, path: &str,
        body: Option<serde_json::Value>, cookie: Option<&str>) -> (StatusCode, reqwest::header::HeaderMap, String) {
        let client = crate::platform::dns::http_client_builder().no_proxy()
            .redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(10)).build().unwrap();
        let mut request = client.request(method, format!("{}{path}", served.base));
        if let Some(body) = body { request = request.json(&body); }
        if let Some(cookie) = cookie { request = request.header(reqwest::header::COOKIE, cookie); }
        let response = request.send().await.unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        let headers = response.headers().clone(); let body = response.text().await.unwrap();
        (status, headers, body)
    }
    fn no_secrets(body: &str) {
        for forbidden in ["fixture-", "SipPwd", "phone_login_token", "application_key", "root_der", "selected_session",
            "private_config", "iccid", "imsi", "__magic"] {
            assert!(!body.contains(forbidden), "redacted response included {forbidden}");
        }
    }
    fn no_store(headers: &reqwest::header::HeaderMap) {
        assert_eq!(headers.get(reqwest::header::CACHE_CONTROL).unwrap(), "no-store");
    }
    fn created_line(body: &str) -> String {
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        let id = json["data"]["line_id"].as_str().unwrap().to_owned();
        assert!(account_store::valid_line_id(&id)); id
    }

    #[tokio::test]
    async fn all_management_and_start_stop_routes_are_behind_real_auth() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await;
        let line = "app-account-0123456789abcdef0123456789abcdef";
        let cases = [
            (reqwest::Method::GET, "/api/non-3gpp/accounts".to_owned()),
            (reqwest::Method::POST, "/api/non-3gpp/accounts".to_owned()),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}")),
            (reqwest::Method::PUT, format!("/api/non-3gpp/accounts/{line}")),
            (reqwest::Method::DELETE, format!("/api/non-3gpp/accounts/{line}?expected_revision=1")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/start")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/stop")),
        ];
        for (method, path) in cases {
            let (status, _, body) = account_request(&served, method, &path, Some(serde_json::json!({})), None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {body}"); no_secrets(&body);
        }
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_metadata_cas_delete_and_private_storage_do_not_start_a_native_session() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let cert = Certificates::generate("DNS:fixture.invalid"); let config = config_json(&cert.root);
        let create = serde_json::json!({"provider_id":"app-voice","config":config});
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts", Some(create), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}"); no_store(&headers); no_secrets(&body);
        let line = created_line(&body); let path = format!("/api/non-3gpp/accounts/{line}");
        let response: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["data"]["phase"], "stopped"); assert_eq!(response["data"]["line_kind"], "app_account");
        assert_eq!(response["data"]["provider_availability"], "unsupported");
        for flag in ["incoming_call_ready","incoming_message_ready","outbound_authorized","io_cleanup_required"] {
            assert_eq!(response["data"][flag], false);
        }
        let stored = state.app.database.with_conn(|conn| account_store::load(conn, &line)).unwrap().unwrap();
        assert_eq!(stored.config.credentials.sdk.phone_login_token.expose_to_backend(), "fixture-original-pwd");
        assert_eq!(stored.config.credentials.hosting.sip_password.expose_to_backend(), "fixture-SipPwd");
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let update = serde_json::json!({"expected_revision":1,"config":config_json(&cert.root)});
        let (status, headers, body) = account_request(&served, reqwest::Method::PUT, &path, Some(update.clone()), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{body}"); no_store(&headers); no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["revision"], 2);
        let (status, headers, body) = account_request(&served, reqwest::Method::PUT, &path, Some(update), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CONFLICT); no_store(&headers); no_secrets(&body);
        // Wrong revision must fail before the factory/connector can do DNS/TCP.
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/start"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CONFLICT); no_store(&headers); no_secrets(&body);
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/stop"),
            Some(serde_json::json!({"expected_revision":2})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let (status, headers, body) = account_request(&served, reqwest::Method::DELETE, &format!("{path}?expected_revision=1"), None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::CONFLICT); no_store(&headers); no_secrets(&body);
        let (status, headers, body) = account_request(&served, reqwest::Method::DELETE, &format!("{path}?expected_revision=2"), None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND); no_store(&headers); no_secrets(&body);
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
        assert!(crate::connectivity::app_voice::registry::registry().is_empty());
    }

    #[tokio::test]
    async fn invalid_secret_unknown_field_provider_and_revision_inputs_are_not_reflected() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let cert = Certificates::generate("DNS:fixture.invalid");
        let mut blank = config_json(&cert.root); blank["credentials"]["sdk"]["phone_login_token"] = serde_json::json!("");
        let mut unknown = config_json(&cert.root); unknown["credentials"]["sdk"]["synthetic-secret-field"] = serde_json::json!("fixture-must-not-echo");
        let mut wrong_type = config_json(&cert.root); wrong_type["profile"]["connect_timeout_ms"] = serde_json::json!("fixture-must-not-echo");
        let mut root = config_json(&cert.root); root["credentials"]["sdk"]["root_der_base64"] = serde_json::json!("fixture-not-a-root");
        for config in [blank, unknown, wrong_type, root] {
            let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
                Some(serde_json::json!({"provider_id":"app-voice","config":config})), Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
            assert!(!body.contains("synthetic-secret-field"));
        }
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"google_voice","config":config_json(&cert.root)})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED); no_store(&headers); no_secrets(&body);
        let line = "app-account-0123456789abcdef0123456789abcdef";
        for body in [serde_json::json!({}), serde_json::json!({"expected_revision":1,"callback":"fixture-must-not-echo"})] {
            let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("/api/non-3gpp/accounts/{line}/start"),
                Some(body), Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        }
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn authenticated_start_and_stop_use_real_native_callbacks_and_original_join() {
        use crate::connectivity::app_voice::providers::jegotrip::{
            account_runtime::AccountManager, account_test_support::{peer, QueuedFactory},
            rpc_envelope::decode_request, runtime_backend::tests::next_non_poll,
            session::{STATUS_REGISTER, STATUS_UNREGISTER},
        };
        let mut state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let cert = Certificates::generate("DNS:fixture.invalid"); let factory = Arc::new(QueuedFactory::default());
        let (mut peer, connector, dropped) = peer(&cert, &factory);
        state.app.app_accounts = Arc::new(AccountManager::with_factory(state.app.database.clone(), factory.clone()));
        let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served = serve(build_router(state.app.clone(), cors)).await; let cookie = authenticate(&served).await;
        let serving = tokio::spawn(async move {
            peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await;
            peer.swap(Some(STATUS_UNREGISTER), true).await;
            let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, crate::connectivity::app_voice::providers::jegotrip::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
            peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
        });
        let (status, _, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app-voice","config":config_json(&cert.root)})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED); let line = created_line(&body);
        let path = format!("/api/non-3gpp/accounts/{line}");
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/start"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}"); no_store(&headers); no_secrets(&body);
        let hosted = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
                assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                if json["data"]["phase"] == "hosted" { break json; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        assert_eq!(hosted["data"]["io_cleanup_required"], true);
        assert_eq!(hosted["data"]["incoming_call_ready"], false); assert_eq!(hosted["data"]["incoming_message_ready"], false);
        assert_eq!(hosted["data"]["outbound_authorized"], false); assert_eq!(hosted["data"]["provider_availability"], "unsupported");
        assert!(connector.captured_scope_is_open());
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/stop"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{body}"); no_store(&headers); no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["io_cleanup_required"], false);
        assert!(!connector.captured_scope_is_open()); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
        serving.await.unwrap();
    }

    #[tokio::test]
    async fn body_limit_blocks_oversized_private_import_and_keeps_storage_empty() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let body = serde_json::json!({"provider_id":"app-voice", "config":"fixture-private-".repeat(32 * 1024)});
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts", Some(body), Some(&cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    }
}
