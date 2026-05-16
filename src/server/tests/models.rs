use super::*;

fn models_snapshot_config(alias: &str) -> crate::config::Config {
    let upstream = redaction_upstream_config(
        "primary",
        "http://127.0.0.1:9/v1",
        crate::formats::UpstreamFormat::OpenAiCompletion,
        None,
        None,
    );
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream],
        model_aliases: BTreeMap::from([(
            alias.to_string(),
            crate::config::ModelAlias {
                upstream_name: "primary".to_string(),
                upstream_model: format!("{alias}-upstream"),
                limits: None,
                surface: None,
            },
        )]),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    }
}

fn models_catalog_config(
    alias: &str,
    upstream_name: &str,
    upstream_model: &str,
    format: crate::formats::UpstreamFormat,
    provider_key: Option<&str>,
) -> crate::config::Config {
    let upstream = redaction_upstream_config(
        upstream_name,
        "http://127.0.0.1:9/v1",
        format,
        None,
        provider_key.map(|secret| crate::config::SecretSourceConfig {
            inline: Some(secret.to_string()),
            env: None,
        }),
    );
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream],
        model_aliases: BTreeMap::from([(
            alias.to_string(),
            crate::config::ModelAlias {
                upstream_name: upstream_name.to_string(),
                upstream_model: upstream_model.to_string(),
                limits: None,
                surface: None,
            },
        )]),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    }
}

fn models_not_found_config(
    format: crate::formats::UpstreamFormat,
    provider_key: Option<&str>,
) -> crate::config::Config {
    crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![
            redaction_upstream_config(
                "left",
                "http://127.0.0.1:9/v1",
                format,
                None,
                provider_key.map(|secret| crate::config::SecretSourceConfig {
                    inline: Some(secret.to_string()),
                    env: None,
                }),
            ),
            redaction_upstream_config(
                "right",
                "http://127.0.0.1:9/v1",
                format,
                None,
                provider_key.map(|secret| crate::config::SecretSourceConfig {
                    inline: Some(secret.to_string()),
                    env: None,
                }),
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
        resource_limits: Default::default(),
        data_auth: None,
    }
}

async fn state_for_models_config(
    config: crate::config::Config,
    access: data_auth::DataAccess,
) -> Arc<AppState> {
    let runtime = crate::server::state::build_runtime_state(config.clone(), &access)
        .await
        .expect("build models snapshot runtime");

    Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        admin_update_lock: Arc::new(Mutex::new(())),
        metrics: crate::telemetry::RuntimeMetrics::new(&config),
        admin_access: AdminAccess::LoopbackOnly,
        data_auth_policy: data_auth::RuntimeConfigValidationPolicy::new(
            "127.0.0.1:0".parse().expect("loopback socket addr"),
            access,
        ),
    })
}

async fn state_for_models_snapshot(alias: &str) -> Arc<AppState> {
    let config = models_snapshot_config(alias);
    let access = data_auth::DataAccess::ClientProviderKey;
    state_for_models_config(config, access).await
}

async fn models_response_text(response: Response<Body>) -> String {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map(|bytes| String::from_utf8(bytes.to_vec()).expect("models response utf8"))
        .expect("models response body")
}

fn assert_models_response_redacted(body_text: &str, secrets: &[&str], context: &str) {
    for secret in secrets {
        assert!(
            !body_text.contains(secret),
            "{context} leaked {secret}: {body_text}"
        );
    }
    assert!(
        body_text.contains("[REDACTED]"),
        "{context} should show redacted placeholder: {body_text}"
    );
}

async fn proxy_mode_models_auth_context(state: &Arc<AppState>) -> data_auth::RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    request_auth_context_for_runtime(
        runtime,
        data_auth::DataAccess::ProxyKey {
            key: PROXY_INLINE_REDACTION_SECRET.to_string(),
        },
        data_auth::RequestAuthorization::ProxyKey,
    )
}

async fn client_mode_models_auth_context(state: &Arc<AppState>) -> data_auth::RequestAuthContext {
    let runtime = state.runtime.read().await.clone();
    request_auth_context_for_runtime(
        runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: CLIENT_PROVIDER_REDACTION_SECRET.to_string(),
        },
    )
}

#[tokio::test(flavor = "current_thread")]
async fn protected_openai_models_use_request_runtime_snapshot_after_auth_race() {
    let state = state_for_models_snapshot("old-model").await;
    let old_runtime = state.runtime.read().await.clone();
    let auth_context = request_auth_context_for_runtime(
        old_runtime,
        data_auth::DataAccess::ClientProviderKey,
        data_auth::RequestAuthorization::ClientProviderKey {
            provider_key: "client-model-key".to_string(),
        },
    );
    replace_runtime_and_data_auth(
        &state,
        models_snapshot_config("new-model"),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;

    let response = crate::server::models::handle_openai_models(
        State(state),
        Some(axum::Extension(auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .map(|bytes| String::from_utf8(bytes.to_vec()).expect("models response utf8"))
        .expect("models response body");
    assert!(body_text.contains("old-model"), "{body_text}");
    assert!(!body_text.contains("new-model"), "{body_text}");
}

#[tokio::test(flavor = "current_thread")]
async fn openai_models_list_redacts_alias_and_metadata_known_secrets() {
    let provider_alias =
        format!("alias-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let provider_upstream =
        format!("upstream-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let provider_model =
        format!("model-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");
    let proxy_access = data_auth::DataAccess::ProxyKey {
        key: PROXY_INLINE_REDACTION_SECRET.to_string(),
    };
    let proxy_state = state_for_models_config(
        models_catalog_config(
            &provider_alias,
            &provider_upstream,
            &provider_model,
            crate::formats::UpstreamFormat::OpenAiCompletion,
            Some(PROVIDER_INLINE_REDACTION_SECRET),
        ),
        proxy_access.clone(),
    )
    .await;
    let proxy_runtime = proxy_state.runtime.read().await.clone();
    let proxy_auth_context = request_auth_context_for_runtime(
        proxy_runtime,
        proxy_access,
        data_auth::RequestAuthorization::ProxyKey,
    );

    let response = crate::server::models::handle_openai_models(
        State(proxy_state),
        Some(axum::Extension(proxy_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[
            PROVIDER_INLINE_REDACTION_SECRET,
            PROXY_INLINE_REDACTION_SECRET,
        ],
        "OpenAI models list proxy mode",
    );
    let body: Value = serde_json::from_str(&body_text).expect("models list JSON");
    let model = &body["data"][0];
    assert_eq!(model["object"], "model");
    assert!(
        model["id"].as_str().unwrap_or("").contains("[REDACTED]"),
        "{body_text}"
    );
    assert!(
        model["llmup"]["upstream_name"]
            .as_str()
            .unwrap_or("")
            .contains("[REDACTED]"),
        "{body_text}"
    );
    assert!(
        model["llmup"]["upstream_model"]
            .as_str()
            .unwrap_or("")
            .contains("[REDACTED]"),
        "{body_text}"
    );

    let client_alias = format!("alias-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_upstream = format!("upstream-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_model = format!("model-{CLIENT_PROVIDER_REDACTION_SECRET}");
    let client_state = state_for_models_config(
        models_catalog_config(
            &client_alias,
            &client_upstream,
            &client_model,
            crate::formats::UpstreamFormat::OpenAiCompletion,
            None,
        ),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let client_auth_context = client_mode_models_auth_context(&client_state).await;

    let response = crate::server::models::handle_openai_models(
        State(client_state),
        Some(axum::Extension(client_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[CLIENT_PROVIDER_REDACTION_SECRET],
        "OpenAI models list client mode",
    );
}

#[tokio::test(flavor = "current_thread")]
async fn openai_model_not_found_redacts_client_and_server_keys() {
    let provider_state = state_for_models_config(
        models_not_found_config(
            crate::formats::UpstreamFormat::OpenAiCompletion,
            Some(PROVIDER_INLINE_REDACTION_SECRET),
        ),
        data_auth::DataAccess::ProxyKey {
            key: PROXY_INLINE_REDACTION_SECRET.to_string(),
        },
    )
    .await;
    let provider_auth_context = proxy_mode_models_auth_context(&provider_state).await;
    let missing_id =
        format!("missing-{PROVIDER_INLINE_REDACTION_SECRET}-{PROXY_INLINE_REDACTION_SECRET}");

    let response = crate::server::models::handle_openai_model(
        State(provider_state),
        Path(missing_id),
        Some(axum::Extension(provider_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[
            PROVIDER_INLINE_REDACTION_SECRET,
            PROXY_INLINE_REDACTION_SECRET,
        ],
        "OpenAI model not found server keys",
    );

    let client_state = state_for_models_config(
        models_not_found_config(crate::formats::UpstreamFormat::OpenAiCompletion, None),
        data_auth::DataAccess::ClientProviderKey,
    )
    .await;
    let client_auth_context = client_mode_models_auth_context(&client_state).await;
    let missing_id = format!("missing-{CLIENT_PROVIDER_REDACTION_SECRET}");

    let response = crate::server::models::handle_openai_model(
        State(client_state),
        Path(missing_id),
        Some(axum::Extension(client_auth_context)),
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body_text = models_response_text(response).await;
    assert_models_response_redacted(
        &body_text,
        &[CLIENT_PROVIDER_REDACTION_SECRET],
        "OpenAI model not found client key",
    );
}
