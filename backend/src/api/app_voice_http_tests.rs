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
    async fn jegotrip_database_routes_require_auth_and_never_export_private_configuration() {
        let state=build_test_router().await.expect("Actions private D-Bus required; no skip");let served=serve(state.router.clone()).await;
        for (method,path) in [
            (reqwest::Method::GET,"/api/jegotrip/config"),(reqwest::Method::PUT,"/api/jegotrip/config"),
            (reqwest::Method::DELETE,"/api/jegotrip/config?expected_revision=0"),(reqwest::Method::GET,"/api/jegotrip/accounts"),
            (reqwest::Method::GET,"/api/jegotrip/login/setup"),(reqwest::Method::GET,"/api/jegotrip/login/attempts"),
            (reqwest::Method::POST,"/api/jegotrip/login/attempts"),
        ] {
            let (status,_,body)=account_request(&served,method,path,Some(serde_json::json!({})),None).await;
            assert_eq!(status,StatusCode::UNAUTHORIZED);no_secrets(&body);
        }
        let cookie=authenticate(&served).await;
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,"/api/jegotrip/config",None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        let value:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(value["data"]["initialized"],false);assert_eq!(value["data"]["revision"],0);
        let (status,headers,body)=account_request(&served,reqwest::Method::PUT,"/api/jegotrip/config",
            Some(serde_json::json!({"expected_revision":0,"configuration":{"version":1,"password":"fixture-secret"}})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }

    #[tokio::test]
    async fn authenticated_jegotrip_configuration_is_database_backed_cas_and_http_only_without_file() {
        let state=build_test_router().await.expect("Actions private D-Bus required; no skip");
        let nonce=crate::connectivity::app_voice::providers::jegotrip::account_runtime::local_nonce().unwrap();
        let lock_path=std::env::temp_dir().join(format!("jegotrip-config-api-{nonce}.lock"));
        state.app.app_accounts.claim_process_ownership(&lock_path).unwrap();
        state.app.app_logins.initialize(std::env::temp_dir().join(format!("missing-jegotrip-{nonce}.json"))).unwrap();
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served=serve(build_router(state.app.clone(),cors)).await;let cookie=authenticate(&served).await;
        let configuration=serde_json::json!({"version":1,"runtime":null,"http":{
            "base_url":"https://fixture-app.invalid/","anonymous_token":"fixture-anonymous","signature_salt":"fixture-signature-salt",
            "password_salt":"fixture-password-salt","sip_password_key":"0123456789abcdefABCDEFGH","device_id":"fixture-device",
            "endpoints":{"login":"login","user_info":"user","phone_token":"csb/token","voip_status":"voip","call_status":"state","enable_hosting":"enable"},
            "body_protection":null,"root_der_base64":null,"user_agent":null,"enroll_id":null}});
        let (status,headers,body)=account_request(&served,reqwest::Method::PUT,"/api/jegotrip/config",Some(serde_json::json!({"expected_revision":0,"configuration":configuration})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        let view:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(view["data"]["configuration_source"],"database");assert_eq!(view["data"]["revision"],1);
        assert_eq!(view["data"]["accepting_authenticate"],true);assert_eq!(view["data"]["accepting_enable_hosting"],false);
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,"/api/jegotrip/login/setup",None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        let view:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(view["data"]["accepting_authenticate"],true);assert_eq!(view["data"]["accepting"],false);
        let (status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/jegotrip/login/attempts",Some(serde_json::json!({
            "attempt_id":"0123456789abcdef0123456789abcdef","username":"fixture-user","password":"fixture-password","country_code":null,
            "intent":"enable_hosting","consent":true,"start_receiver":false})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);assert!(body.contains("app_login_native_configuration_missing"));
        let (status,headers,body)=account_request(&served,reqwest::Method::DELETE,"/api/jegotrip/config?expected_revision=0",None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);
        let (status,headers,body)=account_request(&served,reqwest::Method::DELETE,"/api/jegotrip/config?expected_revision=1",None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        let cleared:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(cleared["data"]["revision"],2);assert_eq!(cleared["data"]["http_configured"],false);
        assert!(state.app.app_logins.list().unwrap().is_empty());assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
        let _=std::fs::remove_file(lock_path);
    }

    #[tokio::test]
    async fn app_login_routes_require_auth_and_setup_exports_no_deployment_or_passwords() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await;
        let attempt = "0123456789abcdef0123456789abcdef";
        for (method, path) in [
            (reqwest::Method::GET, "/api/non-3gpp/app-login/setup".to_owned()),
            (reqwest::Method::GET, "/api/non-3gpp/app-login/attempts".to_owned()),
            (reqwest::Method::POST, "/api/non-3gpp/app-login/attempts".to_owned()),
            (reqwest::Method::GET, format!("/api/non-3gpp/app-login/attempts/{attempt}")),
        ] {
            let (status, _, body) = account_request(&served, method, &path, None, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED); no_secrets(&body);
        }
        let cookie = authenticate(&served).await;
        let (status, headers, body) = account_request(&served, reqwest::Method::GET,
            "/api/non-3gpp/app-login/setup", None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["data"]["implemented"], true);
        assert_eq!(view["data"]["accepting"], false);
        assert_eq!(view["data"]["deployment_present"], false);
        assert_eq!(view["data"]["manual_sdk_credentials_required"], false);
    }

    #[tokio::test]
    async fn app_login_rejects_unknown_fields_and_uninitialized_requests_without_echoing_secrets() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let request = serde_json::json!({"attempt_id":"0123456789abcdef0123456789abcdef", "username":"fixture-user@example.invalid",
            "password":"fixture-password", "country_code":null, "enable_hosting":true, "start_receiver":true});
        let (status, headers, body) = account_request(&served, reqwest::Method::POST,
            "/api/non-3gpp/app-login/attempts", Some(request.clone()), Some(&cookie)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE); no_store(&headers); no_secrets(&body);
        assert!(body.contains("app_login_unavailable"));
        for key in ["phone_login_token", "sip_password", "enrollment_confirmed", "callback", "deployment"] {
            let mut bad = request.clone(); bad[key] = serde_json::json!("fixture-forbidden");
            let (status, headers, body) = account_request(&served, reqwest::Method::POST,
                "/api/non-3gpp/app-login/attempts", Some(bad), Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        }
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
        assert!(state.app.app_logins.list().unwrap().is_empty());
    }

    #[tokio::test]
    async fn credential_metadata_exposes_only_explicit_inbound_implementation_without_start_or_ready_grants() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let (status, headers, body) = account_request(&served, reqwest::Method::GET,
            "/api/non-3gpp/providers/app_voice/credential-contract", None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers);
        assert!(!body.contains("fixture-") && !body.contains("private_config") && !body.contains("__magic"));
        let json: serde_json::Value = serde_json::from_str(&body).unwrap(); let contract = &json["data"];
        assert_eq!(contract["transport_implemented"], true);
        assert_eq!(contract["account_login_ui_required"], false); assert_eq!(contract["service_credentials_required"], true);
        let basic = &contract["basic_hosting"];
        assert_eq!(basic["account_line_kind"], "app_account"); assert_eq!(basic["access"], "application_data");
        assert_eq!(basic["billing_account"], "jegotrip_account"); assert_eq!(basic["manual_start_required"], true);
        assert_eq!(basic["explicit_profile_required"], true); assert_eq!(basic["plain_media_opt_in_required"], true);
        assert_eq!(basic["default_sdk_client_implemented"], false);
        assert_eq!(basic["automatic_cleanup_supported"], true); assert_eq!(basic["automatic_reconnect_implemented"], true);
        assert_eq!(basic["outbound_call_implemented"], false); assert_eq!(basic["outbound_message_implemented"], false);
        assert_eq!(basic["media_transport"], "endpoint_tcp_mpath");
        assert_eq!(basic["media_profile"], "raw_or_zlib_base64_rse_sdp_rtp_avp_rtcp_mux_passthrough");
        assert_eq!(basic["implemented_operations"], serde_json::json!(["incoming_call", "answer_call", "hangup_call", "bidirectional_audio",
            "incoming_message", "message_history", "message_notification", "incoming_trunk"]));
        assert_eq!(contract["direct_sip_interoperability"], "unverified");
        assert_eq!(contract["renewal"], "manual_reimport_until_renewal_verified");
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, "/api/non-3gpp/providers", None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let providers: serde_json::Value = serde_json::from_str(&body).unwrap();
        let rows = providers["data"].as_array().unwrap(); assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row["availability"] == "unsupported" && row["implemented_operations"] == serde_json::json!([])));
        // The HTTP fixture did not run main's explicit installation or create
        // an account. Static implemented metadata cannot mint runtime proof.
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
        assert!(crate::connectivity::app_voice::registry::registry().is_empty());
    }

    #[tokio::test]
    async fn authenticated_cleanup_opt_in_preserves_terminal_state_sms_and_requires_explicit_restart() {
        use crate::connectivity::app_voice::providers::jegotrip::{account_runtime::AccountManager,
            account_test_support::{peer, QueuedFactory}, runtime_backend::tests::next_non_poll, transport_packets::CallKind};
        let mut state = build_test_router().await.expect("Actions private D-Bus required; no skip");
        let cert = Certificates::generate("DNS:fixture.invalid"); let factory = Arc::new(QueuedFactory::default());
        let (mut peer, _, dropped) = peer(&cert, &factory);
        state.app.app_accounts = Arc::new(AccountManager::with_factory(state.app.database.clone(), factory.clone()));
        let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served = serve(build_router(state.app.clone(), cors)).await; let cookie = authenticate(&served).await;
        let mut config = config_json(&cert.root); config["cleanup_on_disconnect"] = serde_json::json!(true);
        config["profile"]["incoming"]["receiver_kind"] = serde_json::json!(b"Sip.Request");
        for bad in [serde_json::json!("true"), serde_json::json!(null), serde_json::json!(1)] {
            let mut invalid = config.clone(); invalid["cleanup_on_disconnect"] = bad;
            let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
                Some(serde_json::json!({"provider_id":"app_voice","config":invalid})), Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        }
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice","config":config})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED); no_store(&headers); no_secrets(&body); let line = created_line(&body);
        let path = format!("/api/non-3gpp/accounts/{line}");
        assert!(state.app.database.with_conn(|conn| account_store::load(conn, &line)).unwrap().unwrap().config.cleanup_on_disconnect);
        assert_eq!(factory.build_count(), 0, "import policy never starts a session");
        let (received, receipt) = tokio::sync::oneshot::channel(); let (disconnect, disconnected) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move { tokio::time::timeout(std::time::Duration::from_secs(8), async move {
            peer.authenticate().await; peer.swap(Some(4), true).await;
            peer.incoming_with_fields(91, b"Sip.Request", b"fixture-preserved-SMS", Some(b"fixture-sender"), None).await;
            let packet = next_non_poll(&mut peer).await; assert_eq!(packet.kind, CallKind::Reply);
            received.send(()).unwrap(); disconnected.await.unwrap(); drop(peer);
        }).await.unwrap() });
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/start"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::ACCEPTED); no_store(&headers); no_secrets(&body);
        tokio::time::timeout(std::time::Duration::from_secs(4), receipt).await.unwrap().unwrap();
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        let active: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(active["data"]["cleanup_on_disconnect"], true); assert_eq!(active["data"]["incoming_message_ready"], true);
        disconnect.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(8), async { loop {
            let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
            assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
            let view: serde_json::Value = serde_json::from_str(&body).unwrap();
            if view["data"]["io_cleanup_required"] == false {
                assert_eq!(view["data"]["phase"], "disconnected"); assert_eq!(view["data"]["last_error"], "app_account_disconnected");
                assert_eq!(view["data"]["cleanup_on_disconnect"], true); assert!(view["data"].get("cleanup_error").is_none());
                for field in ["incoming_message_ready", "incoming_call_ready", "outbound_authorized"] { assert_eq!(view["data"][field], false); }
                break;
            }
            tokio::task::yield_now().await;
        }}).await.unwrap();
        worker.await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, &format!("{path}/messages"), None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"][0]["content"], "fixture-preserved-SMS");
        let held: bool = state.app.database.with_conn(|conn| conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_voice_runtime_leases WHERE line_id=?1)", [&line], |row| row.get(0)
        ).map_err(|_| "test_query_failed")).unwrap();
        assert!(!held, "the original runtime lease is released only after actual cleanup");
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/stop"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        assert_eq!(factory.build_count(), 1);
        let (status, headers, body) = account_request(&served, reqwest::Method::DELETE, &format!("{path}?expected_revision=1"), None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        assert_eq!(state.app.database.get_sms_messages_for_channel(50, 0, None, Some(&line)).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn all_management_and_start_stop_routes_are_behind_real_auth() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await;
        let line = "app-account-0123456789abcdef0123456789abcdef";
        let cases = [
            (reqwest::Method::GET, "/api/non-3gpp/providers".to_owned()),
            (reqwest::Method::GET, "/api/non-3gpp/providers/app_voice/credential-contract".to_owned()),
            (reqwest::Method::GET, "/api/non-3gpp/accounts".to_owned()),
            (reqwest::Method::POST, "/api/non-3gpp/accounts".to_owned()),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}")),
            (reqwest::Method::PUT, format!("/api/non-3gpp/accounts/{line}")),
            (reqwest::Method::DELETE, format!("/api/non-3gpp/accounts/{line}?expected_revision=1")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/start")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/stop")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/inbox")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/messages")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/notifications")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/calls")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/calls/0123456789abcdef0123456789abcdef")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/calls/0123456789abcdef0123456789abcdef/answer")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/calls/0123456789abcdef0123456789abcdef/hangup")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/calls/0123456789abcdef0123456789abcdef/reject")),
            (reqwest::Method::GET, format!("/api/non-3gpp/accounts/{line}/trunk")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/outbound/calls")),
            (reqwest::Method::POST, format!("/api/non-3gpp/accounts/{line}/outbound/messages")),
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
        let create = serde_json::json!({"provider_id":"app_voice","config":config});
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
                Some(serde_json::json!({"provider_id":"app_voice","config":config})), Some(&cookie)).await;
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
    async fn call_and_reserved_outbound_http_are_bounded_revision_guarded_and_never_fallback_to_sim() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let cert = Certificates::generate("DNS:fixture.invalid");
        let (status, _, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})), Some(&cookie)).await;
        assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}");
        let call="0123456789abcdef0123456789abcdef";
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/calls"),None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"],serde_json::json!([]));
        for action in ["answer","hangup","reject"] {
            let request=if action=="answer"{serde_json::json!({"expected_revision":2,"sdp":"fixture-private"})}else{serde_json::json!({"expected_revision":2})};
            let (status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/calls/{call}/{action}"),Some(request),Some(&cookie)).await;
            assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);
            let (status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/calls/{call}/{action}"),Some(serde_json::json!({"expected_revision":1,"private":"fixture-must-not-echo"})),Some(&cookie)).await;
            assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
        }
        for target in ["calls","messages"] {
            let mut request=serde_json::json!({"expected_revision":1,"target":"fixture-private-target"});if target=="messages"{request["body"]=serde_json::json!("fixture-private-body");}
            let (status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/outbound/{target}"),Some(request),Some(&cookie)).await;
            assert_eq!(status,StatusCode::NOT_IMPLEMENTED);no_store(&headers);no_secrets(&body);assert!(body.contains("app_account_outbound_unsupported"));
        }
        assert!(state.app.database.get_call_history_for_line(&line,20,0).unwrap().is_empty());
        assert!(state.app.database.get_sms_messages_for_channel(20,0,None,Some(&line)).unwrap().is_empty());
        assert_eq!(state.app.app_accounts.get(&line,100).await.unwrap().phase,"stopped");
    }

    #[tokio::test]
    async fn actual_authenticated_call_answer_audio_sms_during_call_and_hangup_keep_original_account_and_ack() {
        use crate::connectivity::app_voice::providers::jegotrip as native;
        use native::{account_runtime::AccountManager,account_test_support::{peer,QueuedFactory},
            account_call_test_support::{call_config_json,sdp,notice,operation,reply_operation,finish_sdk,assert_mpath_request,SMS_KIND},
            rpc_calls::{CallNoticeKind,CallOperation,encode_notice_result},rpc_envelope::decode_reply,
            runtime_backend::tests::next_non_poll,tcp_mpath::{self,Input},tcp_mpath_audio::tests::Peer as MediaPeer};
        use tokio::{net::{TcpListener,UdpSocket},sync::oneshot};
        fn redacted_call(body:&str){for secret in ["fixture-original","fixture-domain","#fixture","phone_login_token","application_key","root_der","selected_session","private_config","__magic","SipPwd"]{assert!(!body.contains(secret));}}
        fn rtp(sequence:u16)->Vec<u8>{let mut packet=vec![0x80,0,0,0,0,0,0,160,0x12,0x34,0x56,0x78];packet[2..4].copy_from_slice(&sequence.to_be_bytes());packet.resize(173,sequence as u8);packet}
        let mut state=build_test_router().await.expect("Actions private D-Bus required; do not skip");let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
        let factory=Arc::new(QueuedFactory::default());let(mut sdk_peer,_,dropped)=peer(&sdk,&factory);state.app.app_accounts=Arc::new(AccountManager::with_factory(state.app.database.clone(),factory));
        let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",listener.local_addr().unwrap().port());let audio_peer=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);let served=serve(build_router(state.app.clone(),cors)).await;let cookie=authenticate(&served).await;
        let mut config=call_config_json(&sdk.root,&media.root);config["profile"]["incoming"]["receiver_kind"]=serde_json::json!(SMS_KIND);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::CREATED);no_store(&headers);no_secrets(&body);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}");
        let(go,invite)=oneshot::channel();let(done,stop)=oneshot::channel();let(sms,sms_seen)=oneshot::channel();let observed=state.app.database.clone();let original_line=line.clone();
        let sdk_task=tokio::spawn(async move{tokio::time::timeout(std::time::Duration::from_secs(20),async move{
            sdk_peer.authenticate().await;sdk_peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut sdk_peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;
            let packet=next_non_poll(&mut sdk_peer).await;let reply=decode_reply(packet.payload.expose_to_transport()).unwrap();assert_eq!(reply.call_id,101);assert_eq!(reply.result_stream,encode_notice_result(CallNoticeKind::Invited,true,&[]).unwrap().expose_to_transport());
            let(id,_)=operation(&mut sdk_peer,CallOperation::Inform).await;reply_operation(&mut sdk_peer,id,CallOperation::Inform,true,None).await;
            let(id,_)=operation(&mut sdk_peer,CallOperation::Accept).await;reply_operation(&mut sdk_peer,id,CallOperation::Accept,true,Some(endpoint.as_bytes())).await;
            sdk_peer.incoming_with_fields(201,SMS_KIND,b"fixture simultaneous SMS",Some(b"fixture-sender"),Some(b"1700000000123")).await;
            let packet=next_non_poll(&mut sdk_peer).await;assert_eq!(decode_reply(packet.payload.expose_to_transport()).unwrap().call_id,201);
            let rows=observed.get_sms_messages_for_channel(20,0,None,Some(&original_line)).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].transport,"app_voice");assert_eq!(rows[0].content,"fixture simultaneous SMS");sms.send(()).unwrap();
            let(id,_)=operation(&mut sdk_peer,CallOperation::Release).await;reply_operation(&mut sdk_peer,id,CallOperation::Release,true,None).await;stop.await.unwrap();finish_sdk(sdk_peer).await;
        }).await.unwrap()});
        let down=rtp(33);let up=rtp(34);let native_down=down.clone();let native_up=up.clone();let server=media.server(&tokio_rustls::rustls::version::TLS13);let(passed,saw_audio)=oneshot::channel();
        let media_task=tokio::spawn(async move{tokio::time::timeout(std::time::Duration::from_secs(20),async move{
            let mut peer=MediaPeer::accept(listener,server).await;let packet=peer.next_rpc().await.unwrap().unwrap();assert_mpath_request(packet.payload.expose_to_transport());peer.reply(true).await;
            let(packets,_)=tcp_mpath::encode_payload(0,32,1200,0,&native_down).unwrap();peer.send_raw(packets[0].expose_to_transport()).await;let raw=peer.next_raw().await.unwrap().unwrap();let mut decoder=tcp_mpath::Receiver::default();
            let Some(Input::Payload{channel:0,payload,..})=decoder.on_packet(&raw,std::time::Instant::now()).unwrap()else{panic!("actual HTTP call uplink")};assert_eq!(payload.expose_to_transport(),native_up);passed.send(()).unwrap();peer.eof().await;
        }).await.unwrap()});
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::ACCEPTED);no_store(&headers);no_secrets(&body);
        tokio::time::timeout(std::time::Duration::from_secs(4),async{loop{let(status,headers,body)=account_request(&served,reqwest::Method::GET,&path,None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);let json:serde_json::Value=serde_json::from_str(&body).unwrap();if json["data"]["incoming_call_ready"]==true&&json["data"]["incoming_message_ready"]==true{assert_eq!(json["data"]["outbound_authorized"],false);break;}tokio::task::yield_now().await;}}).await.unwrap();
        go.send(()).unwrap();let call=tokio::time::timeout(std::time::Duration::from_secs(4),async{loop{let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/calls"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);redacted_call(&body);let json:serde_json::Value=serde_json::from_str(&body).unwrap();if json["data"][0]["phase"]=="ringing"{return json["data"][0].clone();}tokio::task::yield_now().await;}}).await.unwrap();
        let id=call["id"].as_str().unwrap();let history_id=call["history_id"].as_i64().unwrap();assert_eq!(call["media_active"],false);let offered=crate::connectivity::core::voice::parse_audio_sdp(call["sdp"].as_str().unwrap().as_bytes()).unwrap();let local:std::net::SocketAddr=format!("{}:{}",offered.connection_addr,offered.media_port).parse().unwrap();
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/calls/{id}/answer"),Some(serde_json::json!({"expected_revision":1,"sdp":std::str::from_utf8(&sdp(audio_peer.local_addr().unwrap())).unwrap()})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK,"{body}");no_store(&headers);redacted_call(&body);let json:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(json["data"]["phase"],"connected");assert_eq!(json["data"]["media_active"],true);
        let mut bytes=[0;4096];let(count,source)=tokio::time::timeout(std::time::Duration::from_secs(4),audio_peer.recv_from(&mut bytes)).await.unwrap().unwrap();assert_eq!(source,local);assert_eq!(&bytes[..count],down);audio_peer.send_to(&up,local).await.unwrap();tokio::time::timeout(std::time::Duration::from_secs(4),saw_audio).await.unwrap().unwrap();tokio::time::timeout(std::time::Duration::from_secs(4),sms_seen).await.unwrap().unwrap();
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/messages"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);let json:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(json["data"][0]["content"],"fixture simultaneous SMS");redacted_call(&body);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/outbound/calls"),Some(serde_json::json!({"expected_revision":1,"target":"fixture-paid-target"})),Some(&cookie)).await;assert_eq!(status,StatusCode::NOT_IMPLEMENTED);no_store(&headers);no_secrets(&body);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/calls/{id}/hangup"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK,"{body}");no_store(&headers);redacted_call(&body);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["media_active"],false);
        media_task.await.unwrap();let reused=UdpSocket::bind(local).await.unwrap();assert_eq!(reused.local_addr().unwrap(),local);let row=state.app.database.get_call_by_id(history_id).unwrap().unwrap();assert!(row.answered);assert!(row.end_time.is_some());assert_eq!(row.line_id.as_deref(),Some(line.as_str()));
        done.send(()).unwrap();let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK,"{body}");no_store(&headers);no_secrets(&body);sdk_task.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(state.app.database.get_sms_messages_for_channel(20,0,None,Some(&line)).unwrap().len(),1);assert!(state.app.database.get_sms_messages_for_channel(20,0,Some("outgoing"),Some(&line)).unwrap().is_empty());
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
            peer.incoming(91).await;
            let reply = next_non_poll(&mut peer).await;
            assert_eq!(reply.kind, crate::connectivity::app_voice::providers::jegotrip::transport_packets::CallKind::Reply);
            assert_eq!(crate::connectivity::app_voice::providers::jegotrip::rpc_envelope::decode_reply(reply.payload.expose_to_transport()).unwrap().call_id, 91);
            peer.swap(Some(STATUS_UNREGISTER), true).await;
            let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, crate::connectivity::app_voice::providers::jegotrip::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
            peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
        });
        let (status, _, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})), Some(&cookie)).await;
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
        let inbox = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let (status, headers, body) = account_request(&served, reqwest::Method::GET, &format!("{path}/inbox?limit=10"), None, Some(&cookie)).await;
                assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
                for private in ["body", "sender", "opaque_time", "nonce", "epoch", "sequence", "fingerprint"] { assert!(!body.contains(private)); }
                let json: serde_json::Value = serde_json::from_str(&body).unwrap();
                if json["data"][0]["reply_state"] == "direct_submitted" { break json; }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        assert_eq!(inbox["data"][0]["classification"], "unclassified");
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, &format!("{path}/stop"),
            Some(serde_json::json!({"expected_revision":1})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{body}"); no_store(&headers); no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["io_cleanup_required"], false);
        assert!(!connector.captured_scope_is_open()); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
        serving.await.unwrap();
    }

    #[tokio::test]
    async fn inbox_metadata_is_authenticated_bounded_and_has_no_raw_export_option() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let cert = Certificates::generate("DNS:fixture.invalid");
        let (status, _, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED); let line = created_line(&body); let path = format!("/api/non-3gpp/accounts/{line}/inbox");
        for suffix in ["?limit=0", "?limit=129", "?limit=invalid", "?include_raw=true", "?body=fixture-must-not-echo"] {
            let (status, headers, body) = account_request(&served, reqwest::Method::GET, &format!("{path}{suffix}"), None, Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        }
        let (status, headers, body) = account_request(&served, reqwest::Method::GET, &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn original_smgw_text_is_readable_in_scoped_sms_history_without_raw_or_credentials() {
        use crate::connectivity::app_voice::providers::jegotrip::{account_runtime::AccountManager,
            account_test_support::{peer,QueuedFactory},rpc_envelope::{decode_reply,decode_request},runtime_backend::tests::next_non_poll,
            session::{STATUS_REGISTER,STATUS_UNREGISTER},transport_packets::CallKind};
        let mut state=build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let cert=Certificates::generate("DNS:fixture.invalid");let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
        state.app.app_accounts=Arc::new(AccountManager::with_factory(state.app.database.clone(),factory));
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served=serve(build_router(state.app.clone(),cors)).await;let cookie=authenticate(&served).await;
        let serving=tokio::spawn(async move {
            peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
            peer.incoming_with_fields(91,b"Sip.Request","fixture SMS 你好 🥘".as_bytes(),Some(b"fixture-sender"),Some(b"1700000000123")).await;
            let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);assert_eq!(decode_reply(reply.payload.expose_to_transport()).unwrap().call_id,91);
            peer.swap(Some(STATUS_UNREGISTER),true).await;let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
        });
        let mut configuration=config_json(&cert.root);configuration["profile"]["incoming"]["receiver_kind"]=serde_json::json!(b"Sip.Request");
        let (status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":configuration})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}");
        let (status,_,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::ACCEPTED,"{body}");
        let projected=tokio::time::timeout(std::time::Duration::from_secs(4),async {
            loop {
                let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/messages?limit=10"),None,Some(&cookie)).await;
                assert_eq!(status,StatusCode::OK);no_store(&headers);
                for private in ["phone_login_token","application_key","root_der","receiver_object","receiver_kind","run_nonce","__magic","private_config"]{assert!(!body.contains(private));}
                let json:serde_json::Value=serde_json::from_str(&body).unwrap();if json["data"].as_array().unwrap().len()==1{break json;}
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        let message=&projected["data"][0];assert_eq!(message["content"],"fixture SMS 你好 🥘");assert_eq!(message["phone_number"],"fixture-sender");assert_eq!(message["transport"],"app_voice");assert!(message["pdu"].is_null());
        assert_eq!(message["classification"],"smgw_text");assert_eq!(message["sent_at_unix_millis"],1_700_000_000_123i64);assert_eq!(message["timestamp_source"],"sender");
        let sms_id=message["id"].as_i64().unwrap();
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,&path,None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        let readiness:serde_json::Value=serde_json::from_str(&body).unwrap();
        assert_eq!(readiness["data"]["incoming_message_ready"],true);assert_eq!(readiness["data"]["provider_availability"],"available");
        assert_eq!(readiness["data"]["incoming_call_ready"],false);assert_eq!(readiness["data"]["outbound_authorized"],false);
        let (status,_,body)=account_request(&served,reqwest::Method::GET,"/api/sms/channels",None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_secrets(&body);
        let channels:serde_json::Value=serde_json::from_str(&body).unwrap();
        let channel=channels["data"].as_array().unwrap().iter().find(|channel|channel["id"]==line).unwrap();
        assert_eq!(channel["available"],true);assert_eq!(channel["incoming_ready"],true);assert_eq!(channel["outbound_authorized"],false);
        assert!(channel.get("iccid").is_none());assert!(channel.get("operator_id").is_none());
        // An actually ready receive channel still cannot send through the
        // modem/IMS billing path or manufacture an outgoing history record.
        let (status,_,body)=account_request(&served,reqwest::Method::POST,&format!("/api/modem/lines/{line}/sms/send"),
            Some(serde_json::json!({"phone_number":"fixture-recipient","content":"must-not-send"})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::NOT_IMPLEMENTED);assert!(body.contains("app_account_outbound_unsupported"));no_secrets(&body);
        assert!(state.app.database.get_sms_messages_for_channel(10,0,Some("outgoing"),Some(&line)).unwrap().is_empty());
        let (status,_,body)=account_request(&served,reqwest::Method::GET,&format!("/api/sms/list?channel_id={line}"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["messages"][0]["id"],sms_id);
        let (status,_,body)=account_request(&served,reqwest::Method::GET,"/api/sms/list?channel_id=another-line",None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["messages"],serde_json::json!([]));
        let (status,_,body)=account_request(&served,reqwest::Method::DELETE,&format!("/api/sms/message/{sms_id}?channel_id={line}"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["deleted"],1);
        let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/messages"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"],serde_json::json!([]));
        let wiped:bool=state.app.database.with_conn(|conn|conn.query_row("SELECT length(body)=0 AND sender IS NULL AND opaque_time IS NULL FROM app_voice_inbox WHERE line_id=?1",[&line],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap();assert!(wiped);
        let (status,_,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK,"{body}");serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn application_sms_channels_never_authorize_local_automation_or_trunk_sends() {
        let state=build_test_router().await.expect("Actions private D-Bus required; do not skip");let served=serve(state.router.clone()).await;let cookie=authenticate(&served).await;
        let cert=Certificates::generate("DNS:fixture.invalid");let (status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})),Some(&cookie)).await;assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);
        let (status,_,body)=account_request(&served,reqwest::Method::GET,"/api/sms/channels",None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_secrets(&body);
        let json:serde_json::Value=serde_json::from_str(&body).unwrap();let channel=json["data"].as_array().unwrap().iter().find(|c|c["id"]==line).unwrap();
        assert_eq!(channel["kind"],"app_account");assert_eq!(channel["available"],false);assert_eq!(channel["incoming_ready"],false);assert_eq!(channel["outbound_authorized"],false);assert!(channel.get("iccid").is_none());assert!(channel.get("operator_id").is_none());
        let (status,_,body)=account_request(&served,reqwest::Method::POST,&format!("/api/modem/lines/{line}/sms/send"),
            Some(serde_json::json!({"phone_number":"fixture-recipient","content":"must-not-send"})),Some(&cookie)).await;assert_eq!(status,StatusCode::NOT_IMPLEMENTED);no_secrets(&body);assert!(body.contains("app_account_outbound_unsupported"));
        assert_eq!(api::handlers::send_sms_on_line(&state.app,&line,"fixture-recipient","must-not-send").await.err().as_deref(),Some("app_account_outbound_unsupported"));
        assert_eq!(api::handlers::send_sms_on_line_from_trunk(&state.app,&line,"fixture-recipient","must-not-send").await.err().as_deref(),Some("app_account_outbound_unsupported"));
        assert!(state.app.database.get_sms_messages_for_channel(50,0,None,Some(&line)).unwrap().is_empty());
        for query in ["?limit=0","?limit=129","?include_raw=true"] {
            let (status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("/api/non-3gpp/accounts/{line}/messages{query}"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
        }
    }

    async fn notification_http_case(change_destination:bool){
        use crate::connectivity::app_voice::providers::jegotrip::{account_runtime::AccountManager,
            account_notifications::NotificationWorker,account_test_support::{peer,QueuedFactory},
            rpc_envelope::decode_request,runtime_backend::tests::next_non_poll,session::{STATUS_REGISTER,STATUS_UNREGISTER},transport_packets::CallKind};
        let mut state=build_test_router().await.expect("Actions private D-Bus required; no skip");
        let (notice_tx,mut notice_rx)=tokio::sync::mpsc::channel::<String>(4);
        let webhook=serve(Router::new().route("/notice",post(move |body:String|{let tx=notice_tx.clone();async move{tx.send(body).await.unwrap();StatusCode::OK}}))).await;
        let cert=Certificates::generate("DNS:fixture.invalid");let factory=Arc::new(QueuedFactory::default());let (mut peer,_,dropped)=peer(&cert,&factory);
        state.app.app_accounts=Arc::new(AccountManager::with_factory_and_notifications(state.app.database.clone(),factory,state.app.notification_sender.clone()));
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served=serve(build_router(state.app.clone(),cors)).await;let cookie=authenticate(&served).await;
        let mut config=config_json(&cert.root);config["profile"]["incoming"]["receiver_kind"]=serde_json::json!(b"Sip.Request");
        let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}");
        let mut notices=serde_json::json!({"version":2,"channels":[{"id":"notice-channel","name":"fixture channel","type":"webhook","enabled":true,"rate_limit":{"enabled":false},"config":{"url":format!("{}/notice",webhook.base),"secret":"","headers":{}}}],
            "rules":[{"id":"notice-rule","name":"fixture rule","type":"sms","enabled":true,"channel_ids":["notice-channel"],"sim_channel_ids":[line.clone()],"template":"{{sim_channel_id}}|{{own_number}}|{{carrier}}|{{content}}"}]});
        state.config_manager.set_notifications(serde_json::from_value(notices.clone()).unwrap()).unwrap();
        let observed=state.app.database.clone();let origin=line.clone();let (received,receipt)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{
            peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
            peer.incoming_with_fields(91,b"Sip.Request",b"fixture-native-message",Some(b"fixture-sender"),None).await;
            let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);
            let jobs:i64=observed.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_notification_jobs WHERE line_id=?1 AND state='pending'",[&origin],|r|r.get(0)).map_err(|_|"test_query_failed")).unwrap();assert_eq!(jobs,1);
            received.send(()).unwrap();peer.swap(Some(STATUS_UNREGISTER),true).await;
            let message=next_non_poll(&mut peer).await;let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
        });
        let(status,_,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::ACCEPTED,"{body}");
        tokio::time::timeout(std::time::Duration::from_secs(5),receipt).await.unwrap().unwrap();assert!(notice_rx.try_recv().is_err());
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/notifications"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"][0]["state"],"pending");
        if change_destination{notices["channels"][0]["config"]["url"]=serde_json::json!(format!("{}/replacement",webhook.base));state.config_manager.set_notifications(serde_json::from_value(notices).unwrap()).unwrap();}
        let(control,signal)=platform::shutdown::channel();let mut worker=NotificationWorker::spawn(state.app.database.clone(),state.app.notification_sender.clone(),signal);
        let expected=if change_destination{"cancelled"}else{"delivered"};
        tokio::time::timeout(std::time::Duration::from_secs(6),async{loop{
            let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/notifications?limit=10"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);
            let json:serde_json::Value=serde_json::from_str(&body).unwrap();if json["data"][0]["state"]==expected{break;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }}).await.unwrap();
        if change_destination{assert!(notice_rx.try_recv().is_err());}else{assert_eq!(notice_rx.recv().await.unwrap(),format!("{line}|||fixture-native-message"));}
        control.trigger();worker.join().await;
        let(status,_,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;assert_eq!(status,StatusCode::OK,"{body}");serving.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&format!("{path}/notifications?include_body=true"),None,Some(&cookie)).await;assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);
    }
    #[tokio::test]
    async fn native_sms_plan_commits_before_ack_then_real_webhook_uses_original_non_sim_context(){notification_http_case(false).await;}
    #[tokio::test]
    async fn changed_destination_under_same_channel_id_cancels_instead_of_rerouting_original_message(){notification_http_case(true).await;}

    fn bootstrap_config_json(source_root: &str, account_root: &str) -> serde_json::Value {
        let mut config = config_json(source_root);
        for field in ["domain_name", "domain_id", "application_id", "device_info_cc"] {
            config["profile"].as_object_mut().unwrap().remove(field);
        }
        config["bootstrap"] = serde_json::json!({
            "account_authority": "account.fixture.invalid", "account_root_der_base64": account_root,
            "account_enable_sni": false, "routing": [{"key": b"ServerGroup", "value": b"original-server-group"}],
            "handoff_timeout_ms": 3000
        });
        config
    }

    #[tokio::test]
    async fn bootstrap_import_stores_separate_original_trust_without_starting_or_exposing_configuration() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let source = Certificates::generate("DNS:fixture.invalid");
        let account = Certificates::generate("DNS:account.fixture.invalid");
        let config = bootstrap_config_json(&source.root, &account.root);
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
            Some(serde_json::json!({"provider_id":"app_voice", "config":config})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}"); no_store(&headers); no_secrets(&body);
        assert!(!body.contains("bootstrap") && !body.contains("account_authority"));
        let line = created_line(&body); let path = format!("/api/non-3gpp/accounts/{line}");
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["data"]["phase"], "stopped"); assert_eq!(view["data"]["io_cleanup_required"], false);
        assert_eq!(view["data"]["incoming_call_ready"], false); assert_eq!(view["data"]["outbound_authorized"], false);
        let stored = state.app.database.with_conn(|conn| account_store::load(conn, &line)).unwrap().unwrap();
        assert!(stored.config.profile.domain_name.is_none() && stored.config.profile.application_id.is_none());
        let bootstrap = stored.config.bootstrap.as_ref().unwrap();
        assert_eq!(bootstrap.account_root_der_base64.expose_to_backend(), account.root);
        assert_eq!(stored.config.credentials.sdk.root_der_base64.expose_to_backend(), source.root);
        assert_eq!(stored.config.credentials.sdk.phone_login_token.expose_to_backend(), "fixture-original-pwd");
        assert_eq!(stored.config.credentials.hosting.sip_password.expose_to_backend(), "fixture-SipPwd");
        let (status, headers, body) = account_request(&served, reqwest::Method::PUT, &path,
            Some(serde_json::json!({"expected_revision":1, "config":config_json(&source.root)})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{body}"); no_store(&headers); no_secrets(&body);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["revision"], 2);
        let replaced = state.app.database.with_conn(|conn| account_store::load(conn, &line)).unwrap().unwrap();
        assert!(replaced.config.bootstrap.is_none()); assert!(replaced.config.profile.domain_name.is_some());
        let (status, headers, body) = account_request(&served, reqwest::Method::DELETE, &format!("{path}?expected_revision=2"), None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK); no_store(&headers); no_secrets(&body);
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
        assert!(crate::connectivity::app_voice::registry::registry().is_empty());
    }

    #[tokio::test]
    async fn ambiguous_or_incomplete_bootstrap_imports_are_rejected_without_secret_reflection_or_session_start() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let cert = Certificates::generate("DNS:fixture.invalid"); let baseline = bootstrap_config_json(&cert.root, &cert.root);
        let mut manual = baseline.clone(); manual["profile"]["domain_name"] = serde_json::json!("manual-placeholder");
        let mut root = baseline.clone(); root["bootstrap"]["account_root_der_base64"] = serde_json::json!("fixture-not-a-root");
        let mut missing = baseline.clone(); missing["bootstrap"].as_object_mut().unwrap().remove("account_root_der_base64");
        let mut authority = baseline.clone(); authority["bootstrap"]["account_authority"] = serde_json::json!("*.fixture.invalid");
        let mut budget = baseline.clone(); budget["bootstrap"]["handoff_timeout_ms"] = serde_json::json!(0);
        let mut routing = baseline.clone(); routing["bootstrap"]["routing"] = serde_json::json!([{"key":b"token", "value":b"not-a-bootstrap-key"}]);
        let mut unknown = baseline; unknown["bootstrap"]["fixture-secret-field"] = serde_json::json!("fixture-must-not-echo");
        for config in [manual, root, missing, authority, budget, routing, unknown] {
            let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts",
                Some(serde_json::json!({"provider_id":"app_voice", "config":config})), Some(&cookie)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}"); no_store(&headers); no_secrets(&body);
            assert!(!body.contains("fixture-secret-field"));
        }
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn other_runtime_owner_is_reported_without_callbacks_or_a_takeover_start_stop_or_delete() {
        use crate::connectivity::app_voice::providers::jegotrip::{account_runtime::AccountManager,
            account_test_support::QueuedFactory,rpc_pending::RpcConnectionScope,session::AttemptOwner,
            runtime_connector::{ConnectFuture,ConnectionInput,ConnectedIo,RuntimeConnector}};
        struct Held{entered:tokio::sync::Notify}
        impl RuntimeConnector for Held {
            fn connect<'a>(&'a self,_owner:&'a AttemptOwner,_scope:&'a RpcConnectionScope,_input:ConnectionInput<'a>)->ConnectFuture<'a>{
                Box::pin(async move{self.entered.notify_one();std::future::pending::<Result<ConnectedIo,crate::connectivity::app_voice::providers::jegotrip::runtime_connector::ConnectError>>().await})
            }
        }
        let mut state=build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let first_factory=Arc::new(QueuedFactory::default());let held=Arc::new(Held{entered:tokio::sync::Notify::new()});first_factory.add(held.clone());
        let original=Arc::new(AccountManager::with_factory(state.app.database.clone(),first_factory));
        let second_factory=Arc::new(QueuedFactory::default());
        state.app.app_accounts=Arc::new(AccountManager::with_factory(state.app.database.clone(),second_factory.clone()));
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        let served=serve(build_router(state.app.clone(),cors)).await;let cookie=authenticate(&served).await;
        let cert=Certificates::generate("DNS:fixture.invalid");
        let(status,_,body)=account_request(&served,reqwest::Method::POST,"/api/non-3gpp/accounts",Some(serde_json::json!({"provider_id":"app_voice","config":config_json(&cert.root)})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::CREATED);let line=created_line(&body);let path=format!("/api/non-3gpp/accounts/{line}");
        let active=original.clone();let id=line.clone();let starting=tokio::spawn(async move{active.start(&id,1,100).await});
        tokio::time::timeout(std::time::Duration::from_secs(2),held.entered.notified()).await.unwrap();
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&path,None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert!(!body.contains("run_nonce"));
        let view:serde_json::Value=serde_json::from_str(&body).unwrap();assert_eq!(view["data"]["io_cleanup_required"],true);
        assert_eq!(view["data"]["last_error"],"app_account_runtime_owner_unconfirmed");assert_eq!(view["data"]["outbound_authorized"],false);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/start"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);
        let(status,headers,body)=account_request(&served,reqwest::Method::POST,&format!("{path}/stop"),Some(serde_json::json!({"expected_revision":1})),Some(&cookie)).await;
        assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);no_store(&headers);no_secrets(&body);
        let(status,headers,body)=account_request(&served,reqwest::Method::DELETE,&format!("{path}?expected_revision=1"),None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::CONFLICT);no_store(&headers);no_secrets(&body);assert_eq!(second_factory.build_count(),0);
        starting.abort();assert!(starting.await.unwrap_err().is_cancelled());original.stop(&line,1,100).await.unwrap();
        let(status,headers,body)=account_request(&served,reqwest::Method::GET,&path,None,Some(&cookie)).await;
        assert_eq!(status,StatusCode::OK);no_store(&headers);no_secrets(&body);assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["io_cleanup_required"],false);
    }

    #[tokio::test]
    async fn body_limit_blocks_oversized_private_import_and_keeps_storage_empty() {
        let state = build_test_router().await.expect("Actions private D-Bus required; do not skip");
        let served = serve(state.router.clone()).await; let cookie = authenticate(&served).await;
        let body = serde_json::json!({"provider_id":"app_voice", "config":"fixture-private-".repeat(32 * 1024)});
        let (status, headers, body) = account_request(&served, reqwest::Method::POST, "/api/non-3gpp/accounts", Some(body), Some(&cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST); no_store(&headers); no_secrets(&body);
        assert!(state.app.app_accounts.list(100).await.unwrap().is_empty());
    }
}
