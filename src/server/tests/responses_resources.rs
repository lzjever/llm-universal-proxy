use super::*;
use crate::server::responses_resources::handle_openai_responses_resource;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn spawn_raw_responses_resource_mock(
    status: StatusCode,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct RawState {
        status: StatusCode,
        body: &'static str,
    }

    async fn handle_resource(State(state): State<RawState>) -> Response<Body> {
        Response::builder()
            .status(state.status)
            .header("Content-Type", "application/json")
            .body(Body::from(state.body))
            .expect("raw resource response")
    }

    let app = Router::new()
        .route(
            "/responses/:id",
            axum::routing::get(handle_resource).delete(handle_resource),
        )
        .route("/responses/:id/cancel", post(handle_resource))
        .route("/responses/compact", post(handle_resource))
        .with_state(RawState { status, body });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw responses resource mock");
    let addr = listener.local_addr().expect("raw resource mock addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("raw resource mock server");
    });

    (format!("http://{addr}"), server)
}

async fn call_raw_responses_resource(
    status: StatusCode,
    upstream_body: &'static str,
    method: reqwest::Method,
    resource_path: &str,
    request_body: Option<Value>,
) -> (Response<Body>, tokio::task::JoinHandle<()>) {
    let (mock_base, server) = spawn_raw_responses_resource_mock(status, upstream_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        method,
        resource_path.to_string(),
        request_body,
        None,
    )
    .await;

    (response, server)
}

async fn spawn_raw_tcp_responses_resource_upstream(
    raw_response: Vec<u8>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw tcp responses resource mock");
    let addr = listener.local_addr().expect("raw tcp resource mock addr");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept raw tcp request");
        let mut request = Vec::new();
        let mut buf = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buf).await.expect("read raw tcp request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buf[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(&raw_response)
            .await
            .expect("write raw tcp response");
        let _ = stream.shutdown().await;
    });

    (format!("http://{addr}"), server)
}

fn raw_no_content_response(status: StatusCode, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let reason = status.canonical_reason().unwrap_or("No Content");
    let mut response = format!(
        "HTTP/1.1 {} {reason}\r\nConnection: close\r\n",
        status.as_u16()
    )
    .into_bytes();
    for (name, value) in headers {
        response.extend_from_slice(name.as_bytes());
        response.extend_from_slice(b": ");
        response.extend_from_slice(value.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    response
}

async fn call_raw_tcp_responses_resource(
    raw_response: Vec<u8>,
) -> (Response<Body>, tokio::task::JoinHandle<()>) {
    let (mock_base, server) = spawn_raw_tcp_responses_resource_upstream(raw_response).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::DELETE,
        "responses/resp_no_content".to_string(),
        None,
        None,
    )
    .await;

    (response, server)
}

async fn response_body_text(response: Response<Body>) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body bytes");
    String::from_utf8(body.to_vec()).expect("response body utf8")
}

async fn assert_raw_no_content_framing_fails_closed(raw_response: Vec<u8>) -> String {
    let (response, server) = call_raw_tcp_responses_resource(raw_response).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned invalid no-content response framing",
        "{body_text}"
    );

    server.abort();
    body_text
}

async fn assert_empty_success_body_fails_closed(
    method: reqwest::Method,
    resource_path: &str,
    request_body: Option<Value>,
) {
    let (response, server) = call_raw_responses_resource(
        StatusCode::OK,
        "",
        method.clone(),
        resource_path,
        request_body,
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "{method} {resource_path}"
    );
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned empty response body",
        "{body_text}"
    );

    server.abort();
}

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
    let responses_no_auto_decompression_client =
        crate::upstream::build_no_auto_decompression_client(
            config.upstream_timeout,
            &responses_resolved_proxy,
        )
        .expect("build responses no-auto-decompression upstream client");
    let (auto_client, auto_streaming_client, auto_resolved_proxy) =
        crate::upstream::build_upstream_clients(
            &config,
            auto.proxy.as_ref(),
            config.proxy.as_ref(),
        )
        .expect("build auto upstream clients");
    let auto_no_auto_decompression_client = crate::upstream::build_no_auto_decompression_client(
        config.upstream_timeout,
        &auto_resolved_proxy,
    )
    .expect("build auto no-auto-decompression upstream client");
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
                no_auto_decompression_client: responses_no_auto_decompression_client,
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
                no_auto_decompression_client: auto_no_auto_decompression_client,
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
    let no_auto_decompression_client = crate::upstream::build_no_auto_decompression_client(
        config.upstream_timeout,
        &resolved_proxy,
    )
    .expect("build responses no-auto-decompression runtime client");
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
                no_auto_decompression_client,
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

#[tokio::test]
async fn handle_openai_responses_resource_invalid_json_success_fails_closed() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        "not json __llmup_custom__secret _llmup_tool_bridge_context",
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_bad_json".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_non_json_error_is_normalized_and_sanitized() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::BAD_REQUEST,
        "upstream failed with __llmup_custom__secret and _llmup_tool_bridge_context",
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("normalized json error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_json_error_without_message_is_sanitized() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::BAD_GATEWAY,
        r#"{"error":{"type":"server_error","debug":"__llmup_custom__secret"},"_llmup_tool_bridge_context":{"entries":{}}}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_error".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("normalized json error");
    assert_eq!(
        body["error"]["message"],
        crate::internal_artifacts::GENERIC_UPSTREAM_ERROR_MESSAGE
    );
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(
        !body_text.contains("_llmup_tool_bridge_context"),
        "{body_text}"
    );
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_success_canonicalizes_validated_json() {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        r#"{"id":"resp_duplicate","object":"response","status":"completed","output":[],"metadata":{"note":"__llmup_custom__secret"},"metadata":{}}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_duplicate".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("canonical json body");
    assert_eq!(body["id"], "resp_duplicate");
    assert_eq!(body["metadata"], serde_json::json!({}));
    assert!(!body_text.contains("__llmup_custom__"), "{body_text}");
    assert!(!body_text.contains("secret"), "{body_text}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_success_preserves_regular_text_metadata_and_schema_descriptions(
) {
    let (mock_base, server) = spawn_raw_responses_resource_mock(
        StatusCode::OK,
        r#"{"id":"resp_plain_reserved_text","object":"response","status":"completed","output":[{"type":"message","id":"msg_plain_reserved_text","role":"assistant","content":[{"type":"output_text","text":"resource output mentions __llmup_custom__apply_patch as plain text","annotations":[]}]}],"metadata":{"note":"user metadata mentions __llmup_custom__apply_patch"},"tools":[{"type":"function","name":"describe_patch_token","description":"schema docs mention __llmup_custom__apply_patch literally","parameters":{"type":"object","properties":{"literal":{"type":"string","description":"schema docs mention __llmup_custom__apply_patch literally"}}}}]}"#,
    )
    .await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiResponses);

    let response = handle_openai_responses_resource(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        reqwest::Method::GET,
        "responses/resp_plain_reserved_text".to_string(),
        None,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("json body bytes");
    let body_text = String::from_utf8(body.to_vec()).expect("json body utf8");
    let body: Value = serde_json::from_str(&body_text).expect("canonical json body");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "resource output mentions __llmup_custom__apply_patch as plain text"
    );
    assert_eq!(
        body["metadata"]["note"],
        "user metadata mentions __llmup_custom__apply_patch"
    );
    assert_eq!(
        body["tools"][0]["parameters"]["properties"]["literal"]["description"],
        "schema docs mention __llmup_custom__apply_patch literally"
    );
    assert!(body_text.contains("__llmup_custom__apply_patch"));

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_200_empty_body_fails_closed_for_lifecycle_methods() {
    for (method, resource_path, request_body) in [
        (reqwest::Method::GET, "responses/resp_empty_get", None),
        (reqwest::Method::DELETE, "responses/resp_empty_delete", None),
        (
            reqwest::Method::POST,
            "responses/resp_empty_cancel/cancel",
            None,
        ),
        (
            reqwest::Method::POST,
            "responses/compact",
            Some(serde_json::json!({
                "input": "compact this response"
            })),
        ),
    ] {
        assert_empty_success_body_fails_closed(method, resource_path, request_body).await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_201_empty_body_fails_closed() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::CREATED,
        "",
        reqwest::Method::GET,
        "responses/resp_empty_created",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body_text = response_body_text(response).await;
    let body: Value = serde_json::from_str(&body_text).expect("OpenAI error envelope");
    assert_eq!(
        body["error"]["message"], "upstream returned empty response body",
        "{body_text}"
    );

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_204_content_length_payload_fails_closed() {
    let body_text = assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "5")],
        b"hello",
    ))
    .await;

    assert!(!body_text.contains("hello"), "{body_text}");
}

#[tokio::test]
async fn handle_openai_responses_resource_204_transfer_encoding_chunked_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Transfer-Encoding", "chunked")],
        b"5\r\nhello\r\n0\r\n\r\n",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_invalid_or_conflicting_content_length_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "0, 5")],
        b"",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_signed_zero_content_length_fails_closed() {
    for content_length in ["+0", "-0", "+000"] {
        assert_raw_no_content_framing_fails_closed(raw_no_content_response(
            StatusCode::NO_CONTENT,
            &[("Content-Length", content_length)],
            b"",
        ))
        .await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_204_zero_content_length_stays_empty() {
    let (response, server) = call_raw_tcp_responses_resource(raw_no_content_response(
        StatusCode::NO_CONTENT,
        &[("Content-Length", "0")],
        b"",
    ))
    .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_205_transfer_encoding_chunked_zero_chunk_fails_closed() {
    assert_raw_no_content_framing_fails_closed(raw_no_content_response(
        StatusCode::RESET_CONTENT,
        &[("Transfer-Encoding", "chunked")],
        b"0\r\n\r\n",
    ))
    .await;
}

#[tokio::test]
async fn handle_openai_responses_resource_204_205_gzip_empty_non_zero_content_length_fails_closed()
{
    let gzip_empty_body =
        b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00";
    for status in [StatusCode::NO_CONTENT, StatusCode::RESET_CONTENT] {
        assert_raw_no_content_framing_fails_closed(raw_no_content_response(
            status,
            &[
                ("Content-Encoding", "gzip"),
                ("Content-Length", &gzip_empty_body.len().to_string()),
            ],
            gzip_empty_body,
        ))
        .await;
    }
}

#[tokio::test]
async fn handle_openai_responses_resource_empty_success_body_stays_empty() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::NO_CONTENT,
        "",
        reqwest::Method::DELETE,
        "responses/resp_empty",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none(),
        "204 empty response should not force JSON content-type"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}

#[tokio::test]
async fn handle_openai_responses_resource_205_empty_success_body_stays_empty() {
    let (response, server) = call_raw_responses_resource(
        StatusCode::RESET_CONTENT,
        "",
        reqwest::Method::DELETE,
        "responses/resp_empty_reset",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::RESET_CONTENT);
    assert!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none(),
        "205 empty response should not force JSON content-type"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("empty body bytes");
    assert!(body.is_empty(), "body = {body:?}");

    server.abort();
}
