use super::*;
use crate::server::responses_resources::handle_openai_responses_resource;

#[test]
fn responses_stateful_request_controls_detect_provider_owned_fields() {
    let controls = responses_stateful_request_controls(&serde_json::json!({
        "previous_response_id": "resp_1",
        "conversation": { "id": "conv_1" },
        "background": true,
        "store": true,
        "prompt": { "id": "pmpt_1" }
    }));

    assert_eq!(
        controls,
        vec![
            "previous_response_id",
            "conversation",
            "background",
            "store",
            "prompt"
        ]
    );
}

#[test]
fn resolve_native_responses_stateful_route_or_error_prefers_unique_available_native_upstream() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        (
            "responses",
            crate::formats::UpstreamFormat::OpenAiResponses,
            true,
        ),
        ("anthropic", crate::formats::UpstreamFormat::Anthropic, true),
    ]);

    let resolved = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect("resolver should succeed")
    .expect("native route should be selected");

    assert_eq!(resolved.upstream_name, "responses");
    assert!(resolved.upstream_model.is_empty());
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_native_upstreams() {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, true),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("multiple native upstreams should fail");

    assert!(error.contains("multiple configured native OpenAI Responses upstreams"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multiple_configured_native_upstreams_even_if_only_one_is_available(
) {
    let namespace_state = runtime_namespace_state_for_tests(&[
        ("a", crate::formats::UpstreamFormat::OpenAiResponses, true),
        ("b", crate::formats::UpstreamFormat::OpenAiResponses, false),
    ]);

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("configured ownership should stay ambiguous");

    assert!(error.contains("multiple"));
    assert!(error.contains("configured"));
    assert!(error.contains("previous_response_id"));
}

#[test]
fn resolve_native_responses_stateful_route_or_error_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin(
) {
    let pinned = test_upstream_config("responses", crate::formats::UpstreamFormat::OpenAiResponses);
    let auto = test_upstream_config_with_fixed_format("auto", None);
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![pinned.clone(), auto.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let (responses_client, responses_streaming_client, responses_resolved_proxy) =
        crate::upstream::build_upstream_clients(
            &config,
            pinned.proxy.as_ref(),
            config.proxy.as_ref(),
        )
        .expect("build responses upstream clients");
    let (auto_client, auto_streaming_client, auto_resolved_proxy) =
        crate::upstream::build_upstream_clients(
            &config,
            auto.proxy.as_ref(),
            config.proxy.as_ref(),
        )
        .expect("build auto upstream clients");
    let upstreams = std::collections::BTreeMap::from([
        (
            "responses".to_string(),
            UpstreamState {
                config: pinned,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client: responses_client,
                streaming_client: responses_streaming_client,
                resolved_proxy: responses_resolved_proxy,
            },
        ),
        (
            "auto".to_string(),
            UpstreamState {
                config: auto,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::Anthropic,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client: auto_client,
                streaming_client: auto_streaming_client,
                resolved_proxy: auto_resolved_proxy,
            },
        ),
    ]);
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams,
        hooks: None,
        debug_trace: None,
    };

    let error = resolve_native_responses_stateful_route_or_error(
        &namespace_state,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "background": true }),
    )
    .expect_err("auto-discovery should block provenance-free routing");

    assert!(error.contains("auto-discovery"));
    assert!(error.contains("fixed_upstream_format"));
}

#[tokio::test]
async fn handle_openai_responses_resource_uses_upstream_state_client() {
    #[derive(Clone)]
    struct ResourceState {
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn handle_compact(
        State(state): State<ResourceState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.bodies.lock().await.push(body);
        Json(serde_json::json!({
            "id": "resp_compact_1",
            "object": "response",
            "status": "completed"
        }))
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/responses/compact", post(handle_compact))
        .with_state(ResourceState {
            bodies: bodies.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind responses resource mock");
    let addr = listener.local_addr().expect("responses resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("responses resource mock server");
    });

    let upstream = crate::config::UpstreamConfig {
        name: "responses".to_string(),
        api_root: format!("http://{addr}"),
        fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
        proxy: None,
        limits: None,
        surface_defaults: None,
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(5),
        compatibility_mode: crate::config::CompatibilityMode::Balanced,
        proxy: Some(crate::config::ProxyConfig::Direct),
        upstreams: vec![upstream.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let (client, streaming_client, resolved_proxy) = crate::upstream::build_upstream_clients(
        &config,
        upstream.proxy.as_ref(),
        config.proxy.as_ref(),
    )
    .expect("build responses runtime clients");
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config: config.clone(),
        upstreams: std::collections::BTreeMap::from([(
            upstream.name.clone(),
            UpstreamState {
                config: upstream,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
                client,
                streaming_client,
                resolved_proxy,
            },
        )]),
        hooks: None,
        debug_trace: None,
    };
    let state = Arc::new(AppState {
        runtime: Arc::new(RwLock::new(RuntimeState {
            namespaces: std::collections::BTreeMap::from([(
                DEFAULT_NAMESPACE.to_string(),
                namespace_state,
            )]),
        })),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
    });

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::POST,
        "responses/compact".to_string(),
        Some(serde_json::json!({ "reasoning": { "effort": "medium" } })),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let recorded = bodies.lock().await;
    assert_eq!(recorded.len(), 1, "bodies = {recorded:?}");
    assert_eq!(recorded[0]["reasoning"]["effort"], "medium");

    server.abort();
}
