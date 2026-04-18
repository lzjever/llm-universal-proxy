use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use super::*;

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<str>) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value.as_ref());
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn test_upstream_config(
    name: &str,
    format: crate::formats::UpstreamFormat,
) -> crate::config::UpstreamConfig {
    test_upstream_config_with_fixed_format(name, Some(format))
}

fn test_upstream_config_with_fixed_format(
    name: &str,
    fixed_upstream_format: Option<crate::formats::UpstreamFormat>,
) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        name: name.to_string(),
        api_root: format!("https://{name}.example/v1"),
        fixed_upstream_format,
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    }
}

fn runtime_namespace_state_for_tests(
    upstreams: &[(&str, crate::formats::UpstreamFormat, bool)],
) -> RuntimeNamespaceState {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: upstreams
            .iter()
            .map(|(name, format, _)| test_upstream_config(name, *format))
            .collect(),
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let upstream_states = upstreams
        .iter()
        .map(|(name, format, available)| {
            (
                (*name).to_string(),
                UpstreamState {
                    config: test_upstream_config(name, *format),
                    capability: Some(crate::discovery::UpstreamCapability::fixed(*format)),
                    availability: if *available {
                        crate::discovery::UpstreamAvailability::available()
                    } else {
                        crate::discovery::UpstreamAvailability::unavailable("test outage")
                    },
                },
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams: upstream_states,
        client: Client::new(),
        hooks: None,
        debug_trace: None,
    }
}

async fn spawn_openai_completion_mock(
    response_body: Value,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    #[derive(Clone)]
    struct MockState {
        requests: Arc<Mutex<Vec<Value>>>,
        response_body: Value,
    }

    async fn handle_chat_completions(
        State(state): State<MockState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(body);
        Json(state.response_body)
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/chat/completions", post(handle_chat_completions))
        .with_state(MockState {
            requests: requests.clone(),
            response_body,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock upstream");
    let addr = listener.local_addr().expect("mock local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock server");
    });

    (format!("http://{addr}"), requests, server)
}

fn app_state_for_single_upstream(
    api_root: String,
    upstream_format: crate::formats::UpstreamFormat,
) -> Arc<AppState> {
    let upstream = crate::config::UpstreamConfig {
        name: "primary".to_string(),
        api_root,
        fixed_upstream_format: Some(upstream_format),
        fallback_credential_env: None,
        fallback_credential_actual: None,
        fallback_api_key: None,
        auth_policy: crate::config::AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![upstream.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let runtime = RuntimeState {
        namespaces: BTreeMap::from([(
            DEFAULT_NAMESPACE.to_string(),
            RuntimeNamespaceState {
                revision: "test-revision".to_string(),
                config: config.clone(),
                upstreams: BTreeMap::from([(
                    upstream.name.clone(),
                    UpstreamState {
                        config: upstream,
                        capability: Some(crate::discovery::UpstreamCapability::fixed(
                            upstream_format,
                        )),
                        availability: crate::discovery::UpstreamAvailability::available(),
                    },
                )]),
                client: crate::upstream::build_client(&config),
                hooks: None,
                debug_trace: None,
            },
        )]),
    };

    Arc::new(AppState {
        runtime: Arc::new(RwLock::new(runtime)),
        metrics: crate::telemetry::RuntimeMetrics::new(&crate::config::Config::default()),
        admin_access: AdminAccess::LoopbackOnly,
    })
}

#[test]
fn extract_forwardable_headers_keeps_only_protocol_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer test"));
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("accept-language", HeaderValue::from_static("*"));
    headers.insert("sec-fetch-mode", HeaderValue::from_static("cors"));

    let forwarded = extract_forwardable_headers(&headers);
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer test"));
    assert!(forwarded
        .iter()
        .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
    assert!(!forwarded.iter().any(|(k, _)| k == "content-type"));
    assert!(!forwarded.iter().any(|(k, _)| k == "accept-language"));
    assert!(!forwarded.iter().any(|(k, _)| k == "sec-fetch-mode"));
}

#[test]
fn normalize_upstream_error_maps_context_window_messages() {
    let error = normalize_upstream_error(
        StatusCode::BAD_REQUEST,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 limit"}}"#,
    );

    assert_eq!(
        error,
        NormalizedUpstreamError {
            message: "prompt is too long: 215000 tokens > 200000 limit".to_string(),
            error_type: "invalid_request_error",
            code: Some("context_length_exceeded"),
        }
    );
}

#[test]
fn normalize_upstream_error_preserves_rate_limit_signal() {
    let error = normalize_upstream_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"Please slow down.","type":"rate_limit_error"}}"#,
    );

    assert_eq!(
        error,
        NormalizedUpstreamError {
            message: "Please slow down.".to_string(),
            error_type: "rate_limit_error",
            code: Some("rate_limit_exceeded"),
        }
    );
}

#[tokio::test]
async fn error_response_anthropic_raw_429_uses_rate_limit_error_and_normalized_message() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"Please slow down.","type":"rate_limit_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], "Please slow down.");
}

#[tokio::test]
async fn error_response_anthropic_raw_401_maps_to_authentication_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"message":"Bad API key.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
    assert_eq!(body["error"]["message"], "Bad API key.");
}

#[tokio::test]
async fn error_response_anthropic_raw_403_maps_to_permission_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::FORBIDDEN,
        r#"{"error":{"message":"Access denied.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "permission_error");
    assert_eq!(body["error"]["message"], "Access denied.");
}

#[tokio::test]
async fn error_response_anthropic_raw_404_maps_to_not_found_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::NOT_FOUND,
        r#"{"error":{"message":"Model not found.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "not_found_error");
    assert_eq!(body["error"]["message"], "Model not found.");
}

#[tokio::test]
async fn error_response_anthropic_raw_413_maps_to_request_too_large() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::PAYLOAD_TOO_LARGE,
        r#"{"error":{"message":"Payload too large.","type":"invalid_request_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "request_too_large");
    assert_eq!(body["error"]["message"], "Payload too large.");
}

#[tokio::test]
async fn error_response_anthropic_raw_503_maps_to_api_error() {
    let response = error_response(
        crate::formats::UpstreamFormat::Anthropic,
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"error":{"message":"Backend overloaded.","type":"server_error"}}"#,
    );

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json body");

    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["message"], "Backend overloaded.");
}

#[test]
fn streaming_error_response_returns_responses_failed_event() {
    let response = streaming_error_response(
        crate::formats::UpstreamFormat::OpenAiResponses,
        StatusCode::BAD_REQUEST,
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}"#,
    );

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let body = runtime.block_on(async move {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    });

    assert!(body.contains("event: response.failed"));
    assert!(body.contains("\"code\":\"context_length_exceeded\""));
    assert!(body.contains("\"message\":\"prompt is too long\""));
}

#[test]
fn normalized_non_stream_upstream_error_does_not_promote_anthropic_context_window_stop() {
    let upstream_body = serde_json::json!({
        "type": "message",
        "stop_reason": "model_context_window_exceeded"
    });

    let actual = normalized_non_stream_upstream_error(
        crate::formats::UpstreamFormat::Anthropic,
        crate::formats::UpstreamFormat::OpenAiResponses,
        &upstream_body,
    );

    assert_eq!(actual, None);
}

#[test]
fn classify_post_translation_non_stream_status_keeps_anthropic_message_success() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "message",
            "content": [{ "type": "text", "text": "Hi" }]
        }),
    );

    assert_eq!(status, StatusCode::OK);
}

#[test]
fn classify_post_translation_non_stream_status_maps_anthropic_tool_error_to_400() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "The provider reported a tool or protocol error."
            }
        }),
    );

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn classify_post_translation_non_stream_status_maps_anthropic_api_error_to_500() {
    let status = classify_post_translation_non_stream_status(
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "type": "error",
            "error": {
                "type": "api_error",
                "message": "The provider returned an error."
            }
        }),
    );

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
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
fn classify_request_boundary_rejects_translated_stateful_responses_controls() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "conversation": { "id": "conv_1" },
            "background": true
        }),
    );

    let RequestBoundaryDecision::Reject(message) = decision else {
        panic!("expected rejection, got {decision:?}");
    };
    assert!(message.contains("conversation"));
    assert!(message.contains("background"));
    assert!(message.contains("native OpenAI Responses"));
}

#[test]
fn classify_request_boundary_keeps_warning_path_for_allowed_degradation() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::OpenAiResponses,
        crate::formats::UpstreamFormat::Anthropic,
        &serde_json::json!({
            "store": true,
            "tools": [{ "type": "web_search" }]
        }),
    );

    let RequestBoundaryDecision::AllowWithWarnings(warnings) = decision else {
        panic!("expected warning path, got {decision:?}");
    };
    assert!(warnings.iter().any(|warning| warning.contains("store")));
    assert!(warnings
        .iter()
        .any(|warning| warning.contains("non-function Responses tools")));
}

#[test]
fn classify_request_boundary_warns_for_gemini_top_k_drop_policy() {
    let decision = classify_request_boundary(
        crate::formats::UpstreamFormat::Google,
        crate::formats::UpstreamFormat::OpenAiCompletion,
        &serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Hi" }]
            }],
            "generationConfig": {
                "topK": 40
            }
        }),
    );

    let RequestBoundaryDecision::AllowWithWarnings(warnings) = decision else {
        panic!("expected warning path, got {decision:?}");
    };
    assert!(warnings.iter().any(|warning| warning.contains("topK")));
}

#[test]
fn append_compatibility_warning_headers_exposes_each_warning() {
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response");
    let warnings = vec![
        "first warning".to_string(),
        "second warning with\nnewline".to_string(),
    ];

    append_compatibility_warning_headers(&mut response, &warnings);

    let values: Vec<_> = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(values, vec!["first warning", "second warning with newline"]);
}

#[tokio::test]
async fn live_responses_store_drop_surfaces_warning_header() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/openai/v1/responses".to_string(),
        serde_json::json!({
            "model": "gpt-4o-mini",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": "Hi" }]
            }],
            "store": true,
            "stream": false
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::OpenAiResponses,
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|warning| warning.contains("store")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0].get("store").is_none(),
        "translated request should drop store: {:?}",
        recorded[0]
    );

    server.abort();
}

#[tokio::test]
async fn live_gemini_top_k_drop_surfaces_warning_header() {
    let response_body = serde_json::json!({
        "id": "chatcmpl_1",
        "object": "chat.completion",
        "created": 123,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Hi" },
            "finish_reason": "stop"
        }]
    });
    let (mock_base, requests, server) = spawn_openai_completion_mock(response_body).await;
    let state =
        app_state_for_single_upstream(mock_base, crate::formats::UpstreamFormat::OpenAiCompletion);

    let response = handle_request_core(
        state,
        DEFAULT_NAMESPACE.to_string(),
        HeaderMap::new(),
        "/google/v1beta/models/gpt-4o-mini:generateContent".to_string(),
        serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Hi" }]
            }],
            "generationConfig": {
                "topK": 40
            }
        }),
        "gpt-4o-mini".to_string(),
        crate::formats::UpstreamFormat::Google,
        Some(false),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|warning| warning.contains("topK")),
        "warnings = {warnings:?}"
    );

    let recorded = requests.lock().await;
    assert_eq!(recorded.len(), 1, "requests = {recorded:?}");
    assert!(
        recorded[0]
            .get("top_k")
            .or_else(|| recorded[0].get("topK"))
            .is_none(),
        "translated request should drop topK: {:?}",
        recorded[0]
    );
    assert!(
        recorded[0]
            .get("generationConfig")
            .and_then(|config| config.get("topK"))
            .is_none(),
        "translated request should drop nested topK: {:?}",
        recorded[0]
    );

    server.abort();
}

#[test]
fn resolve_requested_model_or_error_requires_model_for_multi_upstream_namespace() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({}),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("request must include a routable `model`"));
}

#[test]
fn resolve_requested_model_or_error_explains_previous_response_boundary() {
    let config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![
            crate::config::UpstreamConfig {
                name: "a".to_string(),
                api_root: "https://example.com/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
            crate::config::UpstreamConfig {
                name: "b".to_string(),
                api_root: "https://example.org/v1".to_string(),
                fixed_upstream_format: Some(crate::formats::UpstreamFormat::OpenAiResponses),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: crate::config::AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };

    let error = resolve_requested_model_or_error(
        &config,
        "",
        crate::formats::UpstreamFormat::OpenAiResponses,
        &serde_json::json!({ "previous_response_id": "resp_1" }),
    )
    .expect_err("missing model should fail");

    assert!(error.contains("previous_response_id"));
    assert!(error.contains("does not reconstruct response-to-upstream state"));
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
        upstreams: vec![pinned.clone(), auto.clone()],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    let upstreams = std::collections::BTreeMap::from([
        (
            "responses".to_string(),
            UpstreamState {
                config: pinned,
                capability: Some(crate::discovery::UpstreamCapability::fixed(
                    crate::formats::UpstreamFormat::OpenAiResponses,
                )),
                availability: crate::discovery::UpstreamAvailability::available(),
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
            },
        ),
    ]);
    let namespace_state = RuntimeNamespaceState {
        revision: "test-revision".to_string(),
        config,
        upstreams,
        client: Client::new(),
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

#[tokio::test]
async fn dashboard_runtime_snapshot_tracks_live_namespace_state() {
    let mut config = crate::config::Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: std::time::Duration::from_secs(30),
        upstreams: vec![crate::config::UpstreamConfig {
            name: "auto".to_string(),
            api_root: "https://example.com/v1".to_string(),
            fixed_upstream_format: None,
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: crate::config::AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: crate::config::HookConfig {
            exchange: Some(crate::config::HookEndpointConfig {
                url: "https://example.com/hooks/exchange".to_string(),
                authorization: Some("Bearer hook-1".to_string()),
            }),
            ..Default::default()
        },
        debug_trace: crate::config::DebugTraceConfig::default(),
    };
    config.model_aliases.insert(
        "alias-1".to_string(),
        crate::config::ModelAlias {
            upstream_name: "auto".to_string(),
            upstream_model: "model-a".to_string(),
        },
    );
    let initial_hooks = crate::hooks::HookDispatcher::new(&config.hooks);
    let mut upstreams = BTreeMap::new();
    upstreams.insert(
        "auto".to_string(),
        UpstreamState {
            config: config.upstreams[0].clone(),
            capability: None,
            availability: crate::discovery::UpstreamAvailability::Unavailable {
                reason: "protocol discovery returned no supported formats".to_string(),
            },
        },
    );

    let mut runtime = RuntimeState::default();
    runtime.namespaces.insert(
        DEFAULT_NAMESPACE.to_string(),
        RuntimeNamespaceState {
            revision: "rev-1".to_string(),
            config: config.clone(),
            client: crate::upstream::build_client(&config),
            hooks: initial_hooks,
            debug_trace: None,
            upstreams,
        },
    );

    let handle = DashboardRuntimeHandle::new(Arc::new(RwLock::new(runtime)));
    let snapshot = handle.snapshot();

    assert_eq!(snapshot.config.model_aliases.len(), 1);
    assert_eq!(snapshot.upstreams.len(), 1);
    assert_eq!(snapshot.upstreams[0].name, "auto");
    assert_eq!(snapshot.upstreams[0].availability_status, "unavailable");
    assert_eq!(
        snapshot.upstreams[0].availability_reason.as_deref(),
        Some("protocol discovery returned no supported formats")
    );
    assert!(snapshot.hooks.is_some());

    {
        let mut runtime = handle.runtime.write().await;
        let namespace = runtime
            .namespaces
            .get_mut(DEFAULT_NAMESPACE)
            .expect("default namespace");
        namespace.config.model_aliases.insert(
            "alias-2".to_string(),
            crate::config::ModelAlias {
                upstream_name: "auto".to_string(),
                upstream_model: "model-b".to_string(),
            },
        );
        namespace.upstreams.get_mut("auto").unwrap().availability =
            crate::discovery::UpstreamAvailability::Available;
        namespace.hooks = crate::hooks::HookDispatcher::new(&crate::config::HookConfig::default());
    }

    let updated = handle.snapshot();
    assert_eq!(updated.config.model_aliases.len(), 2);
    assert_eq!(updated.upstreams[0].availability_status, "available");
    assert!(updated.upstreams[0].availability_reason.is_none());
    assert!(updated.hooks.is_none());
}
