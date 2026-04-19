use super::*;

#[test]
fn authorize_admin_request_accepts_matching_bearer_token() {
    let access = AdminAccess::BearerToken("secret-token".to_string());
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer secret-token"),
    );

    assert_eq!(
        authorize_admin_request(
            &access,
            &headers,
            Some("203.0.113.10:8080".parse().unwrap())
        ),
        Ok(())
    );

    let mut lowercase = HeaderMap::new();
    lowercase.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("bearer secret-token"),
    );
    assert_eq!(
        authorize_admin_request(
            &access,
            &lowercase,
            Some("203.0.113.10:8080".parse().unwrap())
        ),
        Ok(())
    );
}

#[test]
fn authorize_admin_request_rejects_missing_or_invalid_bearer_token() {
    let access = AdminAccess::BearerToken("secret-token".to_string());
    let missing = HeaderMap::new();
    assert_eq!(
        authorize_admin_request(&access, &missing, Some("127.0.0.1:8080".parse().unwrap())),
        Err((StatusCode::UNAUTHORIZED, "admin bearer token required"))
    );

    let mut wrong = HeaderMap::new();
    wrong.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer wrong-token"),
    );
    assert_eq!(
        authorize_admin_request(&access, &wrong, Some("127.0.0.1:8080".parse().unwrap())),
        Err((StatusCode::UNAUTHORIZED, "admin bearer token invalid"))
    );
}

#[test]
fn extract_bearer_token_rejects_blank_values() {
    assert_eq!(extract_bearer_token("Bearer "), None);
    assert_eq!(extract_bearer_token("bearer   "), None);
    assert_eq!(extract_bearer_token("Bearer\t"), None);
}

#[test]
fn authorize_admin_request_allows_loopback_only_without_token() {
    let access = AdminAccess::LoopbackOnly;

    assert_eq!(
        authorize_admin_request(
            &access,
            &HeaderMap::new(),
            Some("127.0.0.1:8080".parse().unwrap())
        ),
        Ok(())
    );
    assert_eq!(
        authorize_admin_request(
            &access,
            &HeaderMap::new(),
            Some("[::1]:8080".parse().unwrap())
        ),
        Ok(())
    );
    let mut proxied = HeaderMap::new();
    proxied.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
    assert_eq!(
        authorize_admin_request(&access, &proxied, Some("127.0.0.1:8080".parse().unwrap())),
        Err((
            StatusCode::FORBIDDEN,
            "admin loopback access rejects proxy forwarding headers"
        ))
    );
    assert_eq!(
        authorize_admin_request(
            &access,
            &HeaderMap::new(),
            Some("203.0.113.10:8080".parse().unwrap())
        ),
        Err((
            StatusCode::FORBIDDEN,
            "admin access allowed from loopback clients only"
        ))
    );
    assert_eq!(
        authorize_admin_request(&access, &HeaderMap::new(), None),
        Err((
            StatusCode::FORBIDDEN,
            "admin access allowed from loopback clients only"
        ))
    );
}

#[test]
fn admin_access_from_env_treats_blank_value_as_misconfigured() {
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "   ");

    assert!(matches!(
        AdminAccess::from_env(),
        AdminAccess::Misconfigured
    ));
}

#[test]
fn admin_access_from_env_var_result_treats_not_present_as_loopback_only() {
    assert!(matches!(
        AdminAccess::from_env_var_result(Err(std::env::VarError::NotPresent)),
        AdminAccess::LoopbackOnly
    ));
}

#[cfg(unix)]
#[test]
fn admin_access_from_env_var_result_treats_non_unicode_as_misconfigured() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    assert!(matches!(
        AdminAccess::from_env_var_result(Err(std::env::VarError::NotUnicode(OsString::from_vec(
            vec![0x66, 0x80]
        )))),
        AdminAccess::Misconfigured
    ));
}

#[tokio::test]
async fn admin_namespace_state_sanitizes_urls_and_redacts_sensitive_headers() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![crate::config::UpstreamConfig {
            name: "default".to_string(),
            api_root: "https://user:pass@api.openai.com/v1?api_key=inline-secret#frag".to_string(),
            fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
            fallback_credential_env: Some("DEMO_KEY".to_string()),
            fallback_credential_actual: Some("inline-secret".to_string()),
            fallback_api_key: Some("inline-secret".to_string()),
            auth_policy: crate::config::AuthPolicy::ForceServer,
            upstream_headers: vec![
                ("x-tenant".to_string(), "demo".to_string()),
                (
                    "authorization".to_string(),
                    "Bearer upstream-secret".to_string(),
                ),
            ],
            limits: None,
        }],
        model_aliases: Default::default(),
        hooks: crate::config::HookConfig {
            exchange: Some(crate::config::HookEndpointConfig {
                url: "https://user:pass@example.com/hooks/exchange?token=exchange-secret#frag"
                    .to_string(),
                authorization: Some("Bearer exchange-secret".to_string()),
            }),
            ..crate::config::HookConfig::default()
        },
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let mut upstreams = BTreeMap::new();
    upstreams.insert(
        "default".to_string(),
        UpstreamState {
            config: config.upstreams[0].clone(),
            capability: Some(crate::discovery::UpstreamCapability::fixed(
                crate::formats::UpstreamFormat::OpenAiResponses,
            )),
            availability: crate::discovery::UpstreamAvailability::Available,
        },
    );

    let mut runtime = RuntimeState::default();
    runtime.namespaces.insert(
        "demo".to_string(),
        RuntimeNamespaceState {
            revision: "rev-1".to_string(),
            client: crate::upstream::build_client(&config),
            streaming_client: crate::upstream::build_streaming_client(&config),
            hooks: None,
            debug_trace: None,
            upstreams,
            config,
        },
    );

    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
    });

    let response = handle_admin_namespace_state(State(state), Path("demo".to_string()))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(
        body["config"]["upstreams"][0]["api_root"],
        "https://api.openai.com/v1"
    );
    assert_eq!(
        body["upstreams"][0]["api_root"],
        "https://api.openai.com/v1"
    );
    assert_eq!(
        body["config"]["hooks"]["exchange"]["url"],
        "https://example.com/hooks/exchange"
    );
    assert!(body["config"]["upstreams"][0]["upstream_headers"][1]["value"].is_null());
    assert_eq!(
        body["config"]["upstreams"][0]["upstream_headers"][1]["value_redacted"],
        true
    );
    let body_string = serde_json::to_string(&body).expect("body string");
    assert!(!body_string.contains("user:pass@"));
    assert!(!body_string.contains("inline-secret"));
    assert!(!body_string.contains("exchange-secret"));
    assert!(!body_string.contains("upstream-secret"));
    assert!(!body_string.contains("api_key="));
    assert!(!body_string.contains("token="));
    assert!(!body_string.contains("#frag"));
}
