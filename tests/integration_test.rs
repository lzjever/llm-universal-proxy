//! Full integration tests: proxy + mock upstreams per protocol.
//! Validates passthrough (same format) and translation (different format), non-streaming and streaming.

mod common;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, post},
    Json, Router,
};
use bytes::Bytes;
use common::*;
use futures_util::{future::join_all, stream, StreamExt};
use llm_universal_proxy::config::{
    AuthPolicy, Config, DebugTraceConfig, HookConfig, HookEndpointConfig, ModelAlias,
    RuntimeConfigPayload, RuntimeHookConfig, RuntimeUpstreamConfig, UpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use llm_universal_proxy::server::run_with_listener;
use reqwest::Client;
use serde_json::json;
use serde_json::Value;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

static ADMIN_TOKEN_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static TEST_UPSTREAM_AVAILABILITY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

const TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV: &str =
    "LLM_UNIVERSAL_PROXY_TEST_FORCE_UNAVAILABLE_UPSTREAMS";

type CapturedDiscoveryRequests = Arc<Mutex<Vec<(String, String, String)>>>;
type CapturedGoogleRequests = Arc<Mutex<Vec<(String, Value)>>>;

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

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
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

fn named_upstream(
    name: &str,
    upstream_base: &str,
    format: UpstreamFormat,
    fallback_api_key: Option<&str>,
) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        api_root: upstream_api_root(upstream_base, format),
        fixed_upstream_format: Some(format),
        fallback_credential_env: fallback_api_key.map(|_| format!("{name}_KEY_ENV")),
        fallback_credential_actual: None,
        fallback_api_key: fallback_api_key.map(ToString::to_string),
        auth_policy: AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    }
}

fn config_with_alias(
    upstream_base: &str,
    format: UpstreamFormat,
    alias: &str,
    upstream_model: &str,
) -> Config {
    let mut model_aliases = std::collections::BTreeMap::new();
    model_aliases.insert(
        alias.to_string(),
        ModelAlias {
            upstream_name: "default".to_string(),
            upstream_model: upstream_model.to_string(),
        },
    );
    Config {
        model_aliases,
        ..proxy_config(upstream_base, format)
    }
}

fn demo_runtime_config(mock_base: &str) -> RuntimeConfigPayload {
    RuntimeConfigPayload {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout_secs: 30,
        upstreams: vec![RuntimeUpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(mock_base, UpstreamFormat::OpenAiCompletion),
            fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
            fallback_credential_env: None,
            fallback_credential_actual: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: std::collections::BTreeMap::new(),
        hooks: RuntimeHookConfig::default(),
        debug_trace: DebugTraceConfig::default(),
    }
}

fn auto_discovery_config(upstream_base: &str, api_root_format: UpstreamFormat) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "AUTO".to_string(),
            api_root: upstream_api_root(upstream_base, api_root_format),
            fixed_upstream_format: None,
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    }
}

async fn default_namespace_state(proxy_base: &str) -> Value {
    Client::new()
        .get(format!("{proxy_base}/admin/namespaces/default/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn spawn_tagged_openai_responses_mock(
    tag: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State(tag): State<&'static str>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("missing-model");
        (
            StatusCode::OK,
            Json(json!({
                "id": format!("resp_{tag}"),
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "model": model,
                "output": [
                    {
                        "type": "message",
                        "content": [{ "type": "output_text", "text": format!("hello-from-{tag}") }]
                    }
                ],
                "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 }
            })),
        )
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/responses", post(handler))
        .with_state(tag);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_headered_openai_responses_resource_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(method: Method, uri: Uri) -> Response {
        let body = match (method.as_str(), uri.path()) {
            ("GET", "/v1/responses/resp_123") => json!({
                "id": "resp_123",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": []
            }),
            ("POST", "/v1/responses/resp_123/cancel") => json!({
                "id": "resp_123",
                "object": "response",
                "status": "cancelled",
                "output": []
            }),
            ("POST", "/v1/responses/compact") => json!({
                "id": "resp_compacted",
                "object": "response",
                "created_at": 1,
                "status": "completed",
                "output": []
            }),
            _ => json!({
                "error": {
                    "message": format!("unexpected {} {}", method, uri.path())
                }
            }),
        };
        let status = match (method.as_str(), uri.path()) {
            ("GET", "/v1/responses/resp_123")
            | ("POST", "/v1/responses/resp_123/cancel")
            | ("POST", "/v1/responses/compact") => StatusCode::OK,
            _ => StatusCode::NOT_FOUND,
        };

        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .header("request-id", "req_responses_123")
            .header("openai-processing-ms", "42")
            .header("ratelimit-limit-requests", "99")
            .body(Body::from(
                serde_json::to_vec(&body).expect("serialize responses resource body"),
            ))
            .expect("build responses resource response")
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/responses/compact", any(handler))
        .route("/v1/responses/:response_id", any(handler))
        .route("/v1/responses/:response_id/cancel", any(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_discovery_empty_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedDiscoveryRequests,
) {
    async fn handler(
        State(captured): State<CapturedDiscoveryRequests>,
        method: Method,
        uri: Uri,
        body: String,
    ) -> impl IntoResponse {
        captured
            .lock()
            .unwrap()
            .push((method.to_string(), uri.path().to_string(), body));
        StatusCode::NOT_FOUND
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(any(handler))
        .with_state(captured.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn spawn_openai_completion_terminal_mock(
    finish_reason: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State(finish_reason): State<&'static str>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        (
            StatusCode::OK,
            Json(json!({
                "id": "chatcmpl-terminal",
                "object": "chat.completion",
                "created": 1,
                "model": body.get("model").cloned().unwrap_or_else(|| json!("mock")),
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "" },
                    "finish_reason": finish_reason
                }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 0, "total_tokens": 1 }
            })),
        )
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/chat/completions", post(handler))
        .with_state(finish_reason);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_openai_completion_http_error_mock(
    status: StatusCode,
    body: Value,
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State((status, body)): State<(StatusCode, Value)>,
        Json(_request): Json<Value>,
    ) -> impl IntoResponse {
        (status, Json(body))
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/chat/completions", post(handler))
        .with_state((status, body));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

fn multi_native_responses_config(first_base: &str, second_base: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream(
                "RESPONSES_A",
                first_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
            named_upstream(
                "RESPONSES_B",
                second_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    }
}

fn pinned_responses_plus_auto_discovery_config(
    pinned_base: &str,
    auto_discovery_base: &str,
) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream(
                "RESPONSES_A",
                pinned_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
            UpstreamConfig {
                name: "AUTO".to_string(),
                api_root: auto_discovery_base.to_string(),
                fixed_upstream_format: None,
                fallback_credential_env: None,
                fallback_credential_actual: None,
                fallback_api_key: None,
                auth_policy: AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            },
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    }
}

async fn spawn_google_capture_mock() -> (String, tokio::task::JoinHandle<()>, CapturedGoogleRequests)
{
    async fn handler(
        State(captured): State<CapturedGoogleRequests>,
        Path(model_action): Path<String>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        captured
            .lock()
            .unwrap()
            .push((model_action.clone(), body.clone()));
        if model_action.contains(":streamGenerateContent") {
            let body = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}],"role":"model"},"finishReason":"STOP"}],"modelVersion":"gemini-mock"}"#
                .to_string()
                + "\n\n";
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        } else {
            (
                StatusCode::OK,
                Json(json!({
                    "candidates": [{ "content": { "parts": [{ "text": "Hi" }], "role": "model" }, "finishReason": "STOP" }],
                    "usageMetadata": { "promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2 }
                })),
            )
                .into_response()
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1beta/models/:model_action", post(handler))
        .route("/models/:model_action", post(handler))
        .with_state(captured.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn spawn_google_prompt_feedback_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        Path(_model_action): Path<String>,
        Json(_body): Json<Value>,
    ) -> impl IntoResponse {
        (
            StatusCode::OK,
            Json(json!({
                "promptFeedback": { "blockReason": "SAFETY" },
                "usageMetadata": { "promptTokenCount": 3, "totalTokenCount": 3 },
                "modelVersion": "gemini-2.5"
            })),
        )
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1beta/models/:model_action", post(handler))
        .route("/models/:model_action", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_google_debug_trace_stream_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        Path(model_action): Path<String>,
        Json(_body): Json<Value>,
    ) -> impl IntoResponse {
        if model_action.contains(":streamGenerateContent") {
            let body = concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[",
                "{\"text\":\"Hi\"},",
                "{\"functionCall\":{\"id\":\"call_1\",\"name\":\"lookup_weather\",\"args\":{\"city\":\"Tokyo\"}}}",
                "],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"modelVersion\":\"gemini-debug-trace\"}\n\n"
            );
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        } else {
            (
                StatusCode::OK,
                Json(json!({
                    "candidates": [{
                        "content": {
                            "parts": [
                                { "text": "Hi" },
                                {
                                    "functionCall": {
                                        "id": "call_1",
                                        "name": "lookup_weather",
                                        "args": { "city": "Tokyo" }
                                    }
                                }
                            ],
                            "role": "model"
                        },
                        "finishReason": "STOP"
                    }],
                    "modelVersion": "gemini-debug-trace"
                })),
            )
                .into_response()
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1beta/models/:model_action", post(handler))
        .route("/models/:model_action", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_anthropic_context_window_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(Json(body): Json<Value>) -> impl IntoResponse {
        (
            StatusCode::OK,
            Json(json!({
                "id": "msg_context_window",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": "" }],
                "model": body.get("model").cloned().unwrap_or_else(|| json!("claude-3")),
                "stop_reason": "model_context_window_exceeded",
                "usage": { "input_tokens": 1, "output_tokens": 0 }
            })),
        )
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .route("/messages", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

#[tokio::test]
async fn empty_startup_config_keeps_health_route_available() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let _proxy = tokio::spawn(async move { run_with_listener(Config::default(), listener).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = Client::new();
    let response = client.get(format!("{base}/health")).send().await.unwrap();
    assert!(response.status().is_success());
}

#[tokio::test]
async fn runtime_namespace_config_can_be_created_from_empty_start_with_null_or_missing_if_revision()
{
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let payload = RuntimeConfigPayload {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout_secs: 30,
        upstreams: vec![RuntimeUpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
            fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
            fallback_credential_env: Some("DEMO_KEY".to_string()),
            fallback_credential_actual: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: vec![
                ("x-tenant".to_string(), "demo".to_string()),
                (
                    "authorization".to_string(),
                    "Bearer upstream-secret".to_string(),
                ),
                (
                    "proxy-authorization".to_string(),
                    "Bearer proxy-secret".to_string(),
                ),
                ("cookie".to_string(), "session=secret".to_string()),
                ("set-cookie".to_string(), "session=secret".to_string()),
                ("x-session-token".to_string(), "session-secret".to_string()),
                ("x-api-key".to_string(), "api-secret".to_string()),
            ],
        }],
        model_aliases: std::collections::BTreeMap::new(),
        hooks: RuntimeHookConfig {
            exchange: Some(llm_universal_proxy::config::RuntimeHookEndpointConfig {
                url: "https://example.com/hooks/exchange".to_string(),
                authorization: Some("Bearer exchange-secret".to_string()),
            }),
            usage: Some(llm_universal_proxy::config::RuntimeHookEndpointConfig {
                url: "https://example.com/hooks/usage".to_string(),
                authorization: Some("Bearer usage-secret".to_string()),
            }),
            ..RuntimeHookConfig::default()
        },
        debug_trace: DebugTraceConfig::default(),
    };

    let client = Client::new();
    let apply_with_null = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": null,
            "config": payload,
        }))
        .send()
        .await
        .unwrap();
    assert!(apply_with_null.status().is_success());
    let apply_body: Value = apply_with_null.json().await.unwrap();
    let first_revision = apply_body["revision"].as_str().unwrap().to_string();
    assert_eq!(apply_body["status"], "applied");
    assert!(!first_revision.is_empty());

    let state: Value = client
        .get(format!("{proxy_base}/admin/namespaces/demo/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["revision"], first_revision);
    assert_eq!(state["namespace"], "demo");
    assert_eq!(state["config"]["listen"], "127.0.0.1:0");
    assert_eq!(state["config"]["upstreams"][0]["name"], "default");
    assert_eq!(
        state["config"]["upstreams"][0]["fallback_credential_configured"],
        false
    );

    let apply_missing = client
        .post(format!("{proxy_base}/admin/namespaces/second/config"))
        .json(&json!({
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert!(apply_missing.status().is_success());
    let apply_missing_body: Value = apply_missing.json().await.unwrap();
    let second_revision = apply_missing_body["revision"].as_str().unwrap();
    assert!(!second_revision.is_empty());
    assert_ne!(second_revision, first_revision);

    let res = client
        .post(format!(
            "{proxy_base}/namespaces/demo/openai/v1/chat/completions"
        ))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
}

#[tokio::test]
async fn runtime_namespace_config_updates_with_exact_if_revision_and_generates_new_revision() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let create = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let create_body: Value = create.json().await.unwrap();
    let initial_revision = create_body["revision"].as_str().unwrap().to_string();

    let state: Value = client
        .get(format!("{proxy_base}/admin/namespaces/demo/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["revision"], initial_revision);

    let update = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": initial_revision,
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let update_body: Value = update.json().await.unwrap();
    let next_revision = update_body["revision"].as_str().unwrap().to_string();
    assert_ne!(next_revision, state["revision"].as_str().unwrap());
}

#[tokio::test]
async fn runtime_namespace_config_rejects_stale_or_missing_if_revision_with_412_and_current_revision(
) {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let create = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let create_body: Value = create.json().await.unwrap();
    let initial_revision = create_body["revision"].as_str().unwrap().to_string();

    let update = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": initial_revision,
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let update_body: Value = update.json().await.unwrap();
    let current_revision = update_body["revision"].as_str().unwrap().to_string();

    let stale = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": create_body["revision"],
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);
    let stale_body: Value = stale.json().await.unwrap();
    assert_eq!(stale_body["current_revision"], current_revision);

    let missing = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::PRECONDITION_FAILED);
    let missing_body: Value = missing.json().await.unwrap();
    assert_eq!(missing_body["current_revision"], current_revision);
}

#[tokio::test]
async fn runtime_namespace_config_rejects_non_null_if_revision_when_namespace_does_not_exist() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let response = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": "rev-does-not-exist",
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body: Value = response.json().await.unwrap();
    assert!(body["current_revision"].is_null());
}

#[tokio::test]
async fn default_namespace_startup_config_requires_exact_cas_update() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = Config::try_from(demo_runtime_config(&mock_base)).unwrap();
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let state: Value = client
        .get(format!("{proxy_base}/admin/namespaces/default/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let initial_revision = state["revision"].as_str().unwrap().to_string();
    assert!(!initial_revision.is_empty());
    assert_ne!(initial_revision, "startup");

    let update = client
        .post(format!("{proxy_base}/admin/namespaces/default/config"))
        .json(&json!({
            "if_revision": initial_revision,
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    let update_body: Value = update.json().await.unwrap();
    assert_ne!(update_body["revision"], state["revision"]);
}

#[tokio::test]
async fn runtime_namespace_config_rejects_simultaneous_revision_and_if_revision() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let response = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "revision": "legacy-rev",
            "if_revision": null,
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn runtime_namespace_config_rejects_legacy_revision_shape_with_400() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let response = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "revision": "legacy-rev-1",
            "config": demo_runtime_config(&mock_base),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_namespace_state_redacts_inline_credentials_and_hook_authorization() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let _demo_key = ScopedEnvVar::set("DEMO_KEY", "env-secret");

    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let apply = client
        .post(format!("{proxy_base}/admin/namespaces/demo/config"))
        .json(&json!({
            "if_revision": null,
            "config": RuntimeConfigPayload {
                listen: "127.0.0.1:0".to_string(),
                upstream_timeout_secs: 30,
                upstreams: vec![RuntimeUpstreamConfig {
                    name: "default".to_string(),
                    api_root: upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                    fallback_credential_env: Some("DEMO_KEY".to_string()),
                    fallback_credential_actual: None,
                    auth_policy: AuthPolicy::ForceServer,
                    upstream_headers: vec![
                        ("x-tenant".to_string(), "demo".to_string()),
                        ("authorization".to_string(), "Bearer upstream-secret".to_string()),
                        ("proxy-authorization".to_string(), "Bearer proxy-secret".to_string()),
                        ("cookie".to_string(), "session=secret".to_string()),
                        ("set-cookie".to_string(), "session=secret".to_string()),
                        ("x-session-token".to_string(), "session-secret".to_string()),
                        ("x-api-key".to_string(), "api-secret".to_string()),
                        ("x-client-secret".to_string(), "secret-secret".to_string()),
                        (
                            "x-client-credential".to_string(),
                            "credential-secret".to_string(),
                        ),
                        ("x-service-apikey".to_string(), "apikey-secret".to_string()),
                    ],
                }],
                model_aliases: std::collections::BTreeMap::new(),
                hooks: RuntimeHookConfig {
                    exchange: Some(llm_universal_proxy::config::RuntimeHookEndpointConfig {
                        url: "https://example.com/hooks/exchange".to_string(),
                        authorization: Some("Bearer exchange-secret".to_string()),
                    }),
                    usage: Some(llm_universal_proxy::config::RuntimeHookEndpointConfig {
                        url: "https://example.com/hooks/usage".to_string(),
                        authorization: Some("Bearer usage-secret".to_string()),
                    }),
                    ..RuntimeHookConfig::default()
                },
                debug_trace: DebugTraceConfig::default(),
            },
        }))
        .send()
        .await
        .unwrap();
    assert!(apply.status().is_success());
    let apply_body: Value = apply.json().await.unwrap();
    let applied_revision = apply_body["revision"].as_str().unwrap().to_string();

    let state: Value = client
        .get(format!("{proxy_base}/admin/namespaces/demo/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(state["revision"], applied_revision);
    assert_eq!(
        state["config"]["upstreams"][0]["fallback_credential_env"],
        "DEMO_KEY"
    );
    assert_eq!(
        state["config"]["upstreams"][0]["api_root"],
        upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion)
    );
    assert_eq!(
        state["upstreams"][0]["api_root"],
        upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion)
    );
    assert_eq!(
        state["config"]["upstreams"][0]["fallback_credential_configured"],
        true
    );
    assert_eq!(
        state["config"]["hooks"]["exchange"]["authorization_configured"],
        true
    );
    assert_eq!(
        state["config"]["hooks"]["usage"]["authorization_configured"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][0]["value"],
        "demo"
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][1]["name"],
        "authorization"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][1]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][1]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][2]["name"],
        "proxy-authorization"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][2]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][2]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][3]["name"],
        "cookie"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][3]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][3]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][4]["name"],
        "set-cookie"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][4]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][4]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][5]["name"],
        "x-session-token"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][5]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][5]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][6]["name"],
        "x-api-key"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][6]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][6]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][7]["name"],
        "x-client-secret"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][7]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][7]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][8]["name"],
        "x-client-credential"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][8]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][8]["value_redacted"],
        true
    );
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][9]["name"],
        "x-service-apikey"
    );
    assert!(state["config"]["upstreams"][0]["upstream_headers"][9]["value"].is_null());
    assert_eq!(
        state["config"]["upstreams"][0]["upstream_headers"][9]["value_redacted"],
        true
    );
    assert!(state["config"]["upstreams"][0]
        .get("fallback_credential_actual")
        .is_none());
    assert!(state["config"]["hooks"]["exchange"]
        .get("authorization")
        .is_none());
    assert!(state["config"]["hooks"]["usage"]
        .get("authorization")
        .is_none());

    let body = serde_json::to_string(&state).unwrap();
    assert!(!body.contains("env-secret"));
    assert!(!body.contains("exchange-secret"));
    assert!(!body.contains("usage-secret"));
    assert!(!body.contains("upstream-secret"));
    assert!(!body.contains("session-secret"));
    assert!(!body.contains("proxy-secret"));
    assert!(!body.contains("secret-secret"));
    assert!(!body.contains("credential-secret"));
    assert!(!body.contains("apikey-secret"));
}

#[tokio::test]
async fn admin_routes_require_bearer_token_when_env_is_configured() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "super-secret-token");

    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let client = Client::new();

    let missing = client
        .get(format!("{proxy_base}/admin/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let wrong = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let ok = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer super-secret-token")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);

    let ok_lowercase = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "bearer super-secret-token")
        .send()
        .await
        .unwrap();
    assert_eq!(ok_lowercase.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_routes_fail_closed_when_admin_token_env_is_empty() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::set("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN", "");

    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let client = Client::new();

    let missing = client
        .get(format!("{proxy_base}/admin/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::SERVICE_UNAVAILABLE);

    let wrong = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::SERVICE_UNAVAILABLE);

    let blank = client
        .get(format!("{proxy_base}/admin/state"))
        .header("authorization", "Bearer ")
        .send()
        .await
        .unwrap();
    assert_eq!(blank.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn admin_routes_allow_loopback_when_admin_token_env_is_absent() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");

    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let client = Client::new();

    let response = client
        .get(format!("{proxy_base}/admin/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_routes_reject_proxy_forwarding_headers_in_loopback_mode() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");

    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let client = Client::new();

    for header_name in [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ] {
        let response = client
            .get(format!("{proxy_base}/admin/state"))
            .header(header_name, "203.0.113.10")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{header_name}");
    }
}

#[tokio::test]
async fn admin_routes_do_not_inherit_global_cors_headers() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;
    let client = Client::new();

    let health = client
        .get(format!("{proxy_base}/health"))
        .header("origin", "https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(
        health
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("*")
    );

    let admin = client
        .get(format!("{proxy_base}/admin/state"))
        .header("origin", "https://example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(admin.status(), StatusCode::OK);
    assert!(admin.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn forwarded_headers_whitelist_preserves_protocol_headers_only() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_clone = captured.clone();

    let app = Router::new().route(
        "/v1/messages",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let captured = captured_clone.clone();
            async move {
                *captured.lock().unwrap() = headers
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_string(),
                            value.to_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                let resp = json!({
                    "id": "msg_whitelist",
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "Hi" }],
                    "model": body.get("model").unwrap_or(&json!("claude-3")),
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 1, "output_tokens": 1 }
                });
                (StatusCode::OK, Json(resp)).into_response()
            }
        }),
    );
    let _mock = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let config = proxy_config(&base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;
    let client = Client::new();
    let response = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header("accept-language", "en-US")
        .header("sec-fetch-mode", "cors")
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success());
    let headers = captured.lock().unwrap().clone();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(find("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(
        find("anthropic-beta").as_deref(),
        Some("prompt-caching-2024-07-31")
    );
    assert_eq!(find("accept-language"), None);
    assert_eq!(find("sec-fetch-mode"), None);
}

#[test]
fn config_loads_from_yaml_file() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "llm-universal-proxy-test-{}.yaml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        r#"
listen: 127.0.0.1:9090
upstream_timeout_secs: 33
upstreams:
  GLM-OFFICIAL:
    api_root: https://open.bigmodel.cn/api/anthropic/v1
    format: anthropic
model_aliases:
  GLM-5: GLM-OFFICIAL:GLM-5
"#,
    )
    .unwrap();

    let config = llm_universal_proxy::config::Config::from_yaml_path(&path).unwrap();
    assert_eq!(config.listen, "127.0.0.1:9090");
    assert_eq!(config.upstream_timeout.as_secs(), 33);
    assert_eq!(config.upstreams.len(), 1);
    assert_eq!(config.model_aliases["GLM-5"].upstream_name, "GLM-OFFICIAL");

    let _ = std::fs::remove_file(path);
}

#[test]
fn config_accepts_versionless_absolute_api_root() {
    let config = llm_universal_proxy::config::Config::from_yaml_str(
        r#"
upstreams:
  demo:
    api_root: https://api.openai.com
    format: openai-completion
"#,
    )
    .unwrap();
    assert!(config.validate().is_ok());
}

#[tokio::test]
async fn openai_namespace_chat_completions_works() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
}

#[tokio::test]
async fn openai_namespace_chat_completions_accepts_gzip_upstream_json() {
    async fn gzip_openai_handler() -> Response {
        let compressed = vec![
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 77, 142, 93, 14, 130, 64, 12, 132, 239, 50, 207, 96,
            212, 199, 61, 129, 119, 48, 134, 172, 75, 133, 10, 108, 9, 173, 137, 145, 112, 119,
            139, 241, 239, 169, 201, 124, 51, 157, 153, 193, 53, 2, 82, 27, 45, 13, 99, 95, 54, 15,
            30, 81, 64, 206, 87, 74, 246, 6, 155, 36, 142, 200, 88, 178, 163, 52, 81, 52, 242, 208,
            174, 192, 32, 53, 245, 238, 90, 83, 229, 32, 169, 91, 121, 43, 156, 72, 17, 142, 51,
            56, 215, 116, 71, 216, 186, 147, 84, 99, 67, 8, 51, 38, 233, 253, 34, 170, 178, 90,
            204, 182, 102, 36, 27, 229, 181, 239, 192, 88, 10, 92, 56, 179, 182, 149, 55, 169, 119,
            6, 168, 201, 136, 229, 84, 224, 246, 121, 50, 78, 190, 201, 42, 147, 142, 178, 190,
            182, 252, 70, 254, 171, 38, 22, 251, 175, 176, 95, 150, 39, 28, 44, 142, 26, 241, 0, 0,
            0,
        ];
        Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .header("Content-Encoding", "gzip")
            .body(Body::from(compressed))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route("/v1/chat/completions", post(gzip_openai_handler))
        .route("/chat/completions", post(gzip_openai_handler));
    let _mock = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    let mock_base = format!("http://127.0.0.1:{port}");
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn openai_namespace_responses_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "response");
}

#[tokio::test]
async fn openai_namespace_responses_stream_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body = res.text().await.unwrap();
    assert!(body.contains("response.output_text.delta"));
    assert!(body.contains("response.completed"));
}

#[tokio::test]
async fn openai_namespace_response_get_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_123");
    assert_eq!(body["object"], "response");
}

#[tokio::test]
async fn openai_namespace_response_delete_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .delete(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_123");
    assert_eq!(body["deleted"], true);
}

#[tokio::test]
async fn openai_namespace_response_cancel_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses/resp_123/cancel"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_123");
    assert_eq!(body["status"], "cancelled");
}

#[tokio::test]
async fn openai_namespace_response_compact_works() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses/compact"))
        .json(&json!({ "response_id": "resp_123" }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_compacted");
    assert_eq!(body["object"], "response");
}

#[tokio::test]
async fn openai_namespace_response_resource_routes_preserve_upstream_protocol_headers() {
    let (mock_base, _mock) = spawn_headered_openai_responses_resource_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let responses = vec![
        client
            .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
            .send()
            .await
            .unwrap(),
        client
            .post(format!("{proxy_base}/openai/v1/responses/resp_123/cancel"))
            .send()
            .await
            .unwrap(),
        client
            .post(format!("{proxy_base}/openai/v1/responses/compact"))
            .json(&json!({ "response_id": "resp_123" }))
            .send()
            .await
            .unwrap(),
    ];

    for response in responses {
        assert!(
            response.status().is_success(),
            "status: {}",
            response.status()
        );
        assert_eq!(
            response
                .headers()
                .get("request-id")
                .and_then(|value| value.to_str().ok()),
            Some("req_responses_123")
        );
        assert_eq!(
            response
                .headers()
                .get("openai-processing-ms")
                .and_then(|value| value.to_str().ok()),
            Some("42")
        );
        assert_eq!(
            response
                .headers()
                .get("ratelimit-limit-requests")
                .and_then(|value| value.to_str().ok()),
            Some("99")
        );
    }
}

#[tokio::test]
async fn openai_namespace_response_get_requires_available_native_responses_upstream() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = res.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("available upstream that natively supports OpenAI Responses"));
}

#[tokio::test]
async fn openai_responses_lifecycle_is_ambiguous_with_multiple_native_upstreams() {
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ambiguous"));
}

#[tokio::test]
async fn responses_lifecycle_get_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin()
{
    let (responses_base, _responses_mock) = spawn_openai_responses_mock().await;
    let (auto_base, _auto_mock, _captured) = spawn_discovery_empty_mock().await;
    let config = pinned_responses_plus_auto_discovery_config(&responses_base, &auto_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("auto-discovery"), "message = {message}");
    assert!(
        message.contains("fixed_upstream_format"),
        "message = {message}"
    );
}

#[tokio::test]
async fn responses_compact_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin() {
    let (responses_base, _responses_mock) = spawn_openai_responses_mock().await;
    let (auto_base, _auto_mock, _captured) = spawn_discovery_empty_mock().await;
    let config = pinned_responses_plus_auto_discovery_config(&responses_base, &auto_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses/compact"))
        .json(&json!({ "response_id": "resp_123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("auto-discovery"), "message = {message}");
    assert!(
        message.contains("fixed_upstream_format"),
        "message = {message}"
    );
}

#[tokio::test]
async fn responses_lifecycle_get_is_ambiguous_when_only_one_configured_native_owner_is_available() {
    let _env_guard = TEST_UPSTREAM_AVAILABILITY_ENV_LOCK.lock().await;
    let _forced_unavailable =
        ScopedEnvVar::set(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV, "RESPONSES_B");
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ambiguous"));
}

#[tokio::test]
async fn responses_compact_is_ambiguous_when_only_one_configured_native_owner_is_available() {
    let _env_guard = TEST_UPSTREAM_AVAILABILITY_ENV_LOCK.lock().await;
    let _forced_unavailable =
        ScopedEnvVar::set(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV, "RESPONSES_B");
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses/compact"))
        .json(&json!({ "response_id": "resp_123" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ambiguous"));
}

#[tokio::test]
async fn discovery_empty_result_does_not_masquerade_as_openai_chat_and_returns_503() {
    let (mock_base, _mock, captured) = spawn_discovery_empty_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "AUTO".to_string(),
            api_root: mock_base.clone(),
            fixed_upstream_format: None,
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("unavailable"));
    assert!(message.contains("no supported formats"));

    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 4, "only discovery probes should run");
    assert!(!captured
        .iter()
        .any(|(_, _, body)| body.contains("\"content\":\"Hi\"")));
}

#[tokio::test]
async fn discovery_single_openai_completion_upstream_is_available_and_not_fixed() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = auto_discovery_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let state = default_namespace_state(&proxy_base).await;
    assert_eq!(state["upstreams"][0]["name"], "AUTO");
    assert!(state["upstreams"][0]["fixed_upstream_format"].is_null());
    assert_eq!(
        state["upstreams"][0]["supported_formats"],
        json!(["openai-completion"])
    );
    assert_eq!(state["upstreams"][0]["availability"]["status"], "available");

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn discovery_single_anthropic_upstream_drives_responses_translation() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = auto_discovery_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let state = default_namespace_state(&proxy_base).await;
    assert_eq!(state["upstreams"][0]["name"], "AUTO");
    assert!(state["upstreams"][0]["fixed_upstream_format"].is_null());
    assert_eq!(
        state["upstreams"][0]["supported_formats"],
        json!(["anthropic"])
    );
    assert_eq!(state["upstreams"][0]["availability"]["status"], "available");

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn discovery_single_openai_responses_upstream_allows_lifecycle_success_path() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = auto_discovery_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let state = default_namespace_state(&proxy_base).await;
    assert_eq!(state["upstreams"][0]["name"], "AUTO");
    assert!(state["upstreams"][0]["fixed_upstream_format"].is_null());
    assert_eq!(
        state["upstreams"][0]["supported_formats"],
        json!(["openai-responses"])
    );
    assert_eq!(state["upstreams"][0]["availability"]["status"], "available");

    let res = Client::new()
        .get(format!("{proxy_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_123");
    assert_eq!(body["object"], "response");
}

#[tokio::test]
async fn admin_namespace_state_exposes_unavailable_upstream_discovery_status() {
    let _env_guard = ADMIN_TOKEN_ENV_LOCK.lock().await;
    let _admin_token = ScopedEnvVar::remove("LLM_UNIVERSAL_PROXY_ADMIN_TOKEN");

    let (mock_base, _mock, _captured) = spawn_discovery_empty_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "AUTO".to_string(),
            api_root: mock_base,
            fixed_upstream_format: None,
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let state: Value = Client::new()
        .get(format!("{proxy_base}/admin/namespaces/default/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(state["upstreams"][0]["name"], "AUTO");
    assert_eq!(state["upstreams"][0]["supported_formats"], json!([]));
    assert_eq!(
        state["upstreams"][0]["availability"]["status"],
        "unavailable"
    );
    assert!(state["upstreams"][0]["availability"]["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("no supported formats"));
}

#[tokio::test]
async fn openai_responses_create_with_alias_routes_to_configured_upstream() {
    let (first_base, _first_mock) = spawn_tagged_openai_responses_mock("a").await;
    let (second_base, _second_mock) = spawn_tagged_openai_responses_mock("b").await;
    let mut model_aliases = std::collections::BTreeMap::new();
    model_aliases.insert(
        "resp-a".to_string(),
        ModelAlias {
            upstream_name: "RESPONSES_A".to_string(),
            upstream_model: "model-a".to_string(),
        },
    );
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream(
                "RESPONSES_A",
                &first_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
            named_upstream(
                "RESPONSES_B",
                &second_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
        ],
        model_aliases,
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "resp-a",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_a");
    assert_eq!(body["model"], "model-a");
    assert_eq!(body["output"][0]["content"][0]["text"], "hello-from-a");
}

#[tokio::test]
async fn openai_responses_previous_response_id_requires_explicit_model_in_multi_upstream_namespace()
{
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "input": "Hi again",
            "previous_response_id": "resp_123",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("previous_response_id"));
    assert!(message.contains("routable `model`"));
}

#[tokio::test]
async fn previous_response_id_without_model_is_ambiguous_when_only_one_configured_native_owner_is_available(
) {
    let _env_guard = TEST_UPSTREAM_AVAILABILITY_ENV_LOCK.lock().await;
    let _forced_unavailable =
        ScopedEnvVar::set(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV, "RESPONSES_B");
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "input": "Hi again",
            "previous_response_id": "resp_123",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("previous_response_id"));
    assert!(message.contains("routable `model`"));
}

#[tokio::test]
async fn previous_response_id_without_model_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin(
) {
    let (responses_base, _responses_mock) = spawn_openai_responses_mock().await;
    let (auto_base, _auto_mock, _captured) = spawn_discovery_empty_mock().await;
    let config = pinned_responses_plus_auto_discovery_config(&responses_base, &auto_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "input": "Hi again",
            "previous_response_id": "resp_123",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("auto-discovery"), "message = {message}");
    assert!(
        message.contains("fixed_upstream_format"),
        "message = {message}"
    );
}

#[tokio::test]
async fn background_without_model_is_ambiguous_when_only_one_configured_native_owner_is_available()
{
    let _env_guard = TEST_UPSTREAM_AVAILABILITY_ENV_LOCK.lock().await;
    let _forced_unavailable =
        ScopedEnvVar::set(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV, "RESPONSES_B");
    let (first_base, _first_mock) = spawn_openai_responses_mock().await;
    let (second_base, _second_mock) = spawn_openai_responses_mock().await;
    let config = multi_native_responses_config(&first_base, &second_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "background": true,
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("background"));
    assert!(message.contains("routable `model`"));
}

#[tokio::test]
async fn background_without_model_rejects_multi_upstream_auto_discovery_without_explicit_owner_pin()
{
    let (responses_base, _responses_mock) = spawn_openai_responses_mock().await;
    let (auto_base, _auto_mock, _captured) = spawn_discovery_empty_mock().await;
    let config = pinned_responses_plus_auto_discovery_config(&responses_base, &auto_base);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "background": true,
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("auto-discovery"), "message = {message}");
    assert!(
        message.contains("fixed_upstream_format"),
        "message = {message}"
    );
}

#[tokio::test]
async fn responses_translation_rejects_previous_response_id_without_warning_headers() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "previous_response_id": "resp_123",
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(res.headers().get("x-proxy-compat-warning").is_none());
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("previous_response_id"),
        "message = {message}"
    );
    assert!(
        message.contains("native OpenAI Responses"),
        "message = {message}"
    );
}

#[tokio::test]
async fn responses_translation_rejects_conversation_and_background_stateful_controls() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "conversation": { "id": "conv_123" },
            "background": true,
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    assert!(res.headers().get("x-proxy-compat-warning").is_none());
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("conversation"), "message = {message}");
    assert!(message.contains("background"), "message = {message}");
}

#[tokio::test]
async fn responses_stateful_request_without_model_routes_to_unique_native_responses_upstream() {
    let (responses_base, _responses_mock) = spawn_tagged_openai_responses_mock("a").await;
    let (anthropic_base, _anthropic_mock) = spawn_anthropic_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream(
                "RESPONSES_A",
                &responses_base,
                UpstreamFormat::OpenAiResponses,
                None,
            ),
            named_upstream(
                "ANTHROPIC_B",
                &anthropic_base,
                UpstreamFormat::Anthropic,
                None,
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "previous_response_id": "resp_123",
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "resp_a");
    assert_eq!(body["model"], "missing-model");
}

#[tokio::test]
async fn stateful_model_less_create_returns_503_when_unique_configured_native_owner_is_unavailable()
{
    let _env_guard = TEST_UPSTREAM_AVAILABILITY_ENV_LOCK.lock().await;
    let _forced_unavailable =
        ScopedEnvVar::set(TEST_FORCE_UNAVAILABLE_UPSTREAMS_ENV, "RESPONSES_A");
    let (responses_base, _responses_mock) = spawn_tagged_openai_responses_mock("a").await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![named_upstream(
            "RESPONSES_A",
            &responses_base,
            UpstreamFormat::OpenAiResponses,
            None,
        )],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "previous_response_id": "resp_123",
            "input": "Continue",
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("RESPONSES_A"), "message = {message}");
    assert!(message.contains("unavailable"), "message = {message}");
}

#[tokio::test]
async fn responses_translated_allowed_degradation_emits_warning_headers() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "truncation": "auto",
            "prompt_cache_key": "cache-key",
            "tools": [{ "type": "web_search" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success(), "status: {}", res.status());
    let warnings = res
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("non-function Responses tools")),
        "warnings = {warnings:?}"
    );
}

#[tokio::test]
async fn responses_store_drop_emits_warning_header_on_live_proxy_path() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "store": true,
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success(), "status: {}", res.status());
    let warnings = res
        .headers()
        .get_all("x-proxy-compat-warning")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert!(
        warnings.iter().any(|warning| warning.contains("store")),
        "warnings = {warnings:?}"
    );
}

#[tokio::test]
async fn anthropic_namespace_messages_works() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "message");
}

#[tokio::test]
async fn translated_anthropic_tool_error_returns_400_with_error_body() {
    let (mock_base, _mock) = spawn_openai_completion_terminal_mock("tool_error").await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn translated_anthropic_error_returns_500_with_error_body() {
    let (mock_base, _mock) = spawn_openai_completion_terminal_mock("error").await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
}

#[tokio::test]
async fn translated_anthropic_message_body_stays_200() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "message");
}

#[tokio::test]
async fn anthropic_raw_upstream_429_returns_rate_limit_error() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "error": {
                "message": "Please slow down.",
                "type": "rate_limit_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "rate_limit_error");
    assert_eq!(body["error"]["message"], "Please slow down.");
}

#[tokio::test]
async fn anthropic_raw_upstream_401_returns_authentication_error() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::UNAUTHORIZED,
        json!({
            "error": {
                "message": "Bad API key.",
                "type": "invalid_request_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
    assert_eq!(body["error"]["message"], "Bad API key.");
}

#[tokio::test]
async fn anthropic_raw_upstream_403_returns_permission_error() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::FORBIDDEN,
        json!({
            "error": {
                "message": "Access denied.",
                "type": "invalid_request_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "permission_error");
    assert_eq!(body["error"]["message"], "Access denied.");
}

#[tokio::test]
async fn anthropic_raw_upstream_404_returns_not_found_error() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::NOT_FOUND,
        json!({
            "error": {
                "message": "Model not found.",
                "type": "invalid_request_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "not_found_error");
    assert_eq!(body["error"]["message"], "Model not found.");
}

#[tokio::test]
async fn anthropic_raw_upstream_413_returns_request_too_large() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::PAYLOAD_TOO_LARGE,
        json!({
            "error": {
                "message": "Payload too large.",
                "type": "invalid_request_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "request_too_large");
    assert_eq!(body["error"]["message"], "Payload too large.");
}

#[tokio::test]
async fn anthropic_raw_upstream_503_returns_api_error() {
    let (mock_base, _mock) = spawn_openai_completion_http_error_mock(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({
            "error": {
                "message": "Backend overloaded.",
                "type": "server_error"
            }
        }),
    )
    .await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "max_tokens": 32,
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["message"], "Backend overloaded.");
}

#[tokio::test]
async fn anthropic_namespace_messages_stream_works() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body = res.text().await.unwrap();
    assert!(body.contains("event: message_start"));
    assert!(body.contains("event: message_stop"));
}

#[tokio::test]
async fn native_anthropic_stream_preserves_upstream_protocol_headers() {
    let (mock_base, _mock) = spawn_header_streaming_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success());
    assert_eq!(
        res.headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_upstream_123")
    );
    assert_eq!(
        res.headers()
            .get("anthropic-ratelimit-requests-limit")
            .and_then(|value| value.to_str().ok()),
        Some("99")
    );
}

#[tokio::test]
async fn native_anthropic_non_stream_preserves_upstream_protocol_headers() {
    let (mock_base, _mock) = spawn_headered_anthropic_mock(false, StatusCode::OK).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success());
    assert_eq!(
        res.headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_upstream_123")
    );
    assert_eq!(
        res.headers()
            .get("anthropic-ratelimit-requests-limit")
            .and_then(|value| value.to_str().ok()),
        Some("99")
    );
}

#[tokio::test]
async fn native_anthropic_non_stream_error_preserves_upstream_protocol_headers() {
    let (mock_base, _mock) =
        spawn_headered_anthropic_mock(false, StatusCode::TOO_MANY_REQUESTS).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        res.headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_upstream_123")
    );
    assert_eq!(
        res.headers()
            .get("anthropic-ratelimit-requests-limit")
            .and_then(|value| value.to_str().ok()),
        Some("99")
    );
}

#[tokio::test]
async fn native_anthropic_stream_error_preserves_upstream_protocol_headers() {
    let (mock_base, _mock) =
        spawn_headered_anthropic_mock(true, StatusCode::TOO_MANY_REQUESTS).await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        res.headers()
            .get("request-id")
            .and_then(|value| value.to_str().ok()),
        Some("req_upstream_123")
    );
    assert_eq!(
        res.headers()
            .get("anthropic-ratelimit-requests-limit")
            .and_then(|value| value.to_str().ok()),
        Some("99")
    );
}

#[tokio::test]
async fn google_namespace_generate_content_works() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Google,
        "gemini-local",
        "gemini-1.5",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-local:generateContent"
        ))
        .json(&json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["text"], "Hi");
}

#[tokio::test]
async fn gemini_prompt_feedback_without_candidates_does_not_500() {
    let (mock_base, _mock) = spawn_google_prompt_feedback_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-2.5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["finish_reason"], "content_filter");
    assert_eq!(body["usage"]["prompt_tokens"], 3);
}

#[tokio::test]
async fn google_namespace_stream_generate_content_works() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Google,
        "gemini-local",
        "gemini-1.5",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-local:streamGenerateContent"
        ))
        .json(&json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = res.text().await.unwrap();
    assert!(content_type.contains("text/event-stream"));
    assert!(body.contains("\"candidates\""));
}

#[tokio::test]
async fn google_passthrough_does_not_inject_top_level_stream_field() {
    let (mock_base, _mock, captured) = spawn_google_capture_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Google,
        "gemini-local",
        "gemini-local",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;
    let client = Client::new();

    let non_stream = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-local:generateContent"
        ))
        .json(&json!({
            "contents": [{ "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(
        non_stream.status().is_success(),
        "status: {}",
        non_stream.status()
    );

    let stream = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-local:streamGenerateContent"
        ))
        .json(&json!({
            "contents": [{ "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(stream.status().is_success(), "status: {}", stream.status());
    let _ = stream.text().await.unwrap();

    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 2, "captured = {captured:?}");
    for (_, body) in captured.iter() {
        assert!(body.get("stream").is_none(), "body = {body}");
    }
}

#[tokio::test]
async fn openai_models_endpoint_lists_local_aliases() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::OpenAiCompletion,
        "sonnet",
        "gpt-4o",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/openai/v1/models"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert_eq!(body["data"][0]["id"], "sonnet");
    assert_eq!(body["data"][0]["proxec"]["upstream_model"], "gpt-4o");
}

#[tokio::test]
async fn anthropic_models_endpoint_retrieves_local_alias() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Anthropic,
        "haiku",
        "claude-3-haiku",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/anthropic/v1/models/haiku"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "haiku");
    assert_eq!(body["type"], "model");
}

#[tokio::test]
async fn google_models_endpoint_lists_local_aliases() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Google,
        "flash",
        "gemini-2.0-flash",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/google/v1beta/models"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["models"][0]["name"], "models/flash");
    assert_eq!(
        body["models"][0]["supportedGenerationMethods"][0],
        "generateContent"
    );
}

#[tokio::test]
async fn upstream_openai_completion_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert!(body.get("choices").and_then(|c| c.get(0)).is_some());
    assert_eq!(body["choices"][0]["message"]["content"], "Hi"); // mock returns "Hi"
}

#[tokio::test]
async fn openai_completion_omitted_stream_defaults_to_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let ct = res
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !ct.contains("event-stream"),
        "default stream should be false"
    );
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
}

#[tokio::test]
async fn upstream_openai_completion_client_anthropic_translated_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    // Client sends Anthropic format (system + messages) → proxy translates to OpenAI for upstream, then response back to Anthropic shape.
    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "system": "You are helpful.",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("content").and_then(|c| c.as_array()).is_some());
    assert_eq!(body["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn upstream_anthropic_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 100,
            "system": "You are helpful.",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body.get("content").and_then(|c| c.as_array()).is_some());
    assert_eq!(body["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn upstream_anthropic_client_openai_translated_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn anthropic_messages_endpoint_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "claude-3",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn anthropic_messages_endpoint_translates_to_openai_upstream() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gpt-4",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn responses_endpoint_translates_to_anthropic_upstream_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "Hi");
}

#[tokio::test]
async fn responses_endpoint_preserves_anthropic_reasoning_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["output"][0]["type"], "reasoning");
    assert_eq!(body["output"][0]["summary"][0]["text"], "think");
    assert_eq!(body["output"][1]["type"], "message");
    assert_eq!(body["usage"]["output_tokens"], 2);
}

#[tokio::test]
async fn upstream_google_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Google,
        "gemini-1.5",
        "gemini-1.5",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-1.5:generateContent"
        ))
        .json(&json!({
            "contents": [{ "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    // Passthrough: response is native Gemini format
    assert!(body.get("candidates").and_then(|c| c.get(0)).is_some());
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["text"], "Hi");
}

#[tokio::test]
async fn anthropic_context_window_exceeded_non_stream_stays_on_success_path() {
    let (mock_base, _mock) = spawn_anthropic_context_window_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let res = Client::new()
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(
        body["choices"][0]["finish_reason"],
        "context_length_exceeded"
    );
}

#[tokio::test]
async fn upstream_openai_responses_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4",
            "input": [{ "type": "message", "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["usage"]["input_tokens"], 1);
    assert_eq!(body["usage"]["output_tokens"], 1);
    let output = body["output"].as_array().unwrap();
    let msg = output.iter().find(|o| o["type"] == "message").unwrap();
    let text_part = msg["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["type"] == "output_text")
        .unwrap();
    assert_eq!(text_part["text"], "Hi");
}

#[derive(Clone, Default)]
struct CapturedHeaders {
    headers: Arc<Mutex<Vec<(String, String)>>>,
}

#[derive(Clone, Default)]
struct CapturedAnthropicRequests {
    requests: Arc<Mutex<Vec<CapturedAnthropicRequest>>>,
}

#[derive(Clone, Debug)]
struct CapturedAnthropicRequest {
    headers: Vec<(String, String)>,
    body: Value,
}

#[derive(Clone, Copy)]
struct HeaderedAnthropicMockConfig {
    expected_stream: bool,
    status: StatusCode,
}

async fn spawn_header_capture_anthropic_mock(
) -> (String, tokio::task::JoinHandle<()>, CapturedHeaders) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let state = CapturedHeaders::default();
    let app = Router::new()
        .route("/v1/messages", post(capture_anthropic_handler))
        .route("/messages", post(capture_anthropic_handler))
        .with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, state)
}

async fn capture_anthropic_handler(
    State(state): State<CapturedHeaders>,
    headers: HeaderMap,
    Json(_body): Json<Value>,
) -> impl axum::response::IntoResponse {
    let captured = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect::<Vec<_>>();
    *state.headers.lock().unwrap() = captured;
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "Hi" }],
            "model": "claude-3",
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })),
    )
}

async fn spawn_concurrent_capture_anthropic_mock() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedAnthropicRequests,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let state = CapturedAnthropicRequests::default();
    let app = Router::new()
        .route("/v1/messages", post(capture_concurrent_anthropic_handler))
        .route("/messages", post(capture_concurrent_anthropic_handler))
        .with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, state)
}

async fn capture_concurrent_anthropic_handler(
    State(state): State<CapturedAnthropicRequests>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl axum::response::IntoResponse {
    let captured_headers = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect::<Vec<_>>();
    state
        .requests
        .lock()
        .unwrap()
        .push(CapturedAnthropicRequest {
            headers: captured_headers,
            body,
        });
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "id": "msg_concurrent",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "Hi" }],
            "model": "claude-3",
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })),
    )
}

#[derive(Clone, Default)]
struct CapturedAuthRequests {
    requests: Arc<Mutex<Vec<CapturedAnthropicRequest>>>,
}

#[derive(Clone, Default)]
struct CapturedHookPayloads {
    payloads: Arc<Mutex<Vec<Value>>>,
}

async fn spawn_auth_capture_anthropic_mock(
) -> (String, tokio::task::JoinHandle<()>, CapturedAuthRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let state = CapturedAuthRequests::default();
    let app = Router::new()
        .route("/v1/messages", post(capture_auth_anthropic_handler))
        .route("/messages", post(capture_auth_anthropic_handler))
        .with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, state)
}

async fn capture_auth_anthropic_handler(
    State(state): State<CapturedAuthRequests>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl axum::response::IntoResponse {
    let captured_headers = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect::<Vec<_>>();
    state
        .requests
        .lock()
        .unwrap()
        .push(CapturedAnthropicRequest {
            headers: captured_headers,
            body,
        });
    (
        axum::http::StatusCode::OK,
        Json(json!({
            "id": "msg_auth",
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "text", "text": "Hi" }],
            "model": "claude-3",
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })),
    )
}

async fn spawn_header_streaming_anthropic_mock() -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(Json(body): Json<Value>) -> Response {
        let stream_enabled = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        assert!(stream_enabled, "expected streaming anthropic request");

        let pieces = vec![
            Ok::<Bytes, std::io::Error>(Bytes::from_static(
                br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_headers","type":"message","role":"assistant","model":"claude-3","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}

"#,
            )),
            Ok(Bytes::from_static(
                br#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":2}}

"#,
            )),
            Ok(Bytes::from_static(
                br#"event: message_stop
data: {"type":"message_stop"}

"#,
            )),
        ];
        let body_stream = stream::iter(pieces);
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .header("request-id", "req_upstream_123")
            .header("anthropic-ratelimit-requests-limit", "99")
            .body(Body::from_stream(body_stream))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .route("/messages", post(handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_headered_anthropic_mock(
    expected_stream: bool,
    status: StatusCode,
) -> (String, tokio::task::JoinHandle<()>) {
    async fn handler(
        State(config): State<HeaderedAnthropicMockConfig>,
        Json(body): Json<Value>,
    ) -> Response {
        let stream_enabled = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        assert_eq!(
            stream_enabled, config.expected_stream,
            "unexpected stream flag for anthropic request"
        );

        let response_body = if config.status.is_success() {
            json!({
                "id": "msg_headers",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": "Hi" }],
                "model": "claude-3",
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 1, "output_tokens": 1 }
            })
        } else {
            json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "message": "Too many requests."
                }
            })
        };

        Response::builder()
            .status(config.status)
            .header("Content-Type", "application/json")
            .header("request-id", "req_upstream_123")
            .header("anthropic-ratelimit-requests-limit", "99")
            .body(Body::from(
                serde_json::to_vec(&response_body).expect("serialize anthropic mock response"),
            ))
            .unwrap()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/messages", post(handler))
        .route("/messages", post(handler))
        .with_state(HeaderedAnthropicMockConfig {
            expected_stream,
            status,
        });
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn spawn_hook_capture_server() -> (String, tokio::task::JoinHandle<()>, CapturedHookPayloads)
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let state = CapturedHookPayloads::default();
    let app = Router::new()
        .route("/hook", post(capture_hook_handler))
        .with_state(state.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, state)
}

async fn capture_hook_handler(
    State(state): State<CapturedHookPayloads>,
    Json(body): Json<Value>,
) -> impl axum::response::IntoResponse {
    state.payloads.lock().unwrap().push(body);
    (axum::http::StatusCode::OK, Json(json!({"ok": true})))
}

async fn spawn_slow_openai_completion_mock() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let app = Router::new()
        .route("/v1/chat/completions", post(slow_openai_completion_handler))
        .route("/chat/completions", post(slow_openai_completion_handler));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle)
}

async fn slow_openai_completion_handler(Json(body): Json<Value>) -> Response {
    let stream_enabled = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if !stream_enabled {
        return (
            axum::http::StatusCode::OK,
            Json(json!({
                "id": "chatcmpl-slow",
                "object": "chat.completion",
                "created": 1,
                "model": body.get("model").unwrap_or(&json!("mock")),
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": "Hi" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            })),
        )
            .into_response();
    }

    let pieces = vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(
            br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
        )),
        Ok(Bytes::from_static(b"\n\n")),
        Ok(Bytes::from_static(
            br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"#,
        )),
        Ok(Bytes::from_static(b"\n\n")),
        Ok(Bytes::from_static(
            br#"data: {"id":"chatcmpl-slow","object":"chat.completion.chunk","created":1,"model":"mock","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )),
        Ok(Bytes::from_static(b"\n\n")),
        Ok(Bytes::from_static(b"data: [DONE]\n\n")),
    ];
    let body_stream = stream::unfold(pieces.into_iter().enumerate(), |mut iter| async move {
        if let Some((idx, chunk)) = iter.next() {
            if idx >= 2 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Some((chunk, iter))
        } else {
            None
        }
    });
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .body(Body::from_stream(body_stream))
        .unwrap()
}

#[tokio::test]
async fn upstream_anthropic_injects_required_version_header() {
    let (mock_base, _mock, captured) = spawn_header_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());

    let headers = captured.headers.lock().unwrap();
    let version = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
        .map(|(_, value)| value.clone());
    assert_eq!(version.as_deref(), Some("2023-06-01"));
}

#[tokio::test]
async fn multi_upstream_supports_explicit_upstream_model_selector() {
    let (glm_base, _glm_mock) = spawn_anthropic_mock().await;
    let (openai_base, _openai_mock) = spawn_openai_completion_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream("GLM-OFFICIAL", &glm_base, UpstreamFormat::Anthropic, None),
            named_upstream(
                "OPENAI",
                &openai_base,
                UpstreamFormat::OpenAiCompletion,
                None,
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "GLM-OFFICIAL:GLM-5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn multi_upstream_supports_local_model_alias() {
    let (glm_base, _glm_mock) = spawn_anthropic_mock().await;
    let (openai_base, _openai_mock) = spawn_openai_completion_mock().await;
    let mut model_aliases = std::collections::BTreeMap::new();
    model_aliases.insert(
        "GLM-5".to_string(),
        ModelAlias {
            upstream_name: "GLM-OFFICIAL".to_string(),
            upstream_model: "GLM-5".to_string(),
        },
    );
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream("GLM-OFFICIAL", &glm_base, UpstreamFormat::Anthropic, None),
            named_upstream(
                "OPENAI",
                &openai_base,
                UpstreamFormat::OpenAiCompletion,
                None,
            ),
        ],
        model_aliases,
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "GLM-5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn multi_upstream_requires_explicit_resolution_for_ambiguous_model() {
    let (glm_base, _glm_mock) = spawn_anthropic_mock().await;
    let (openai_base, _openai_mock) = spawn_openai_completion_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![
            named_upstream("GLM-OFFICIAL", &glm_base, UpstreamFormat::Anthropic, None),
            named_upstream(
                "OPENAI",
                &openai_base,
                UpstreamFormat::OpenAiCompletion,
                None,
            ),
        ],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "shared-model",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 400);
}

#[tokio::test]
async fn multi_upstream_uses_per_upstream_fallback_credential() {
    let (glm_base, _mock, captured) = spawn_auth_capture_anthropic_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![named_upstream(
            "GLM-OFFICIAL",
            &glm_base,
            UpstreamFormat::Anthropic,
            Some("glm-secret"),
        )],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "GLM-OFFICIAL:GLM-5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());

    let requests = captured.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let api_key = requests[0]
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        .map(|(_, value)| value.as_str());
    assert_eq!(api_key, Some("glm-secret"));
}

#[tokio::test]
async fn force_server_auth_policy_ignores_client_key() {
    let (glm_base, _mock, captured) = spawn_auth_capture_anthropic_mock().await;
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "GLM-OFFICIAL".to_string(),
            api_root: upstream_api_root(&glm_base, UpstreamFormat::Anthropic),
            fixed_upstream_format: Some(UpstreamFormat::Anthropic),
            fallback_credential_env: None,
            fallback_credential_actual: Some("server-secret".to_string()),
            fallback_api_key: Some("server-secret".to_string()),
            auth_policy: AuthPolicy::ForceServer,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .header("authorization", "Bearer client-secret")
        .json(&json!({
            "model": "GLM-OFFICIAL:GLM-5",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());

    let requests = captured.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let api_key = requests[0]
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        .map(|(_, value)| value.as_str());
    assert_eq!(api_key, Some("server-secret"));
}

#[tokio::test]
async fn usage_and_exchange_hooks_fire_for_non_streaming_requests() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .header("authorization", "Bearer client-secret")
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    tokio::time::sleep(Duration::from_millis(100)).await;

    let payloads = captured.payloads.lock().unwrap();
    assert_eq!(payloads.len(), 2);
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(exchange["request"]["body"]["messages"][0]["content"], "Hi");
    assert_eq!(
        exchange["response"]["body"]["choices"][0]["message"]["content"],
        "Hi"
    );
    assert_eq!(exchange["credential_source"], "client");
    assert!(exchange["credential_fingerprint"].as_str().unwrap().len() == 16);

    let usage = payloads
        .iter()
        .find(|payload| payload.get("usage").is_some())
        .unwrap();
    assert_eq!(usage["usage"]["input_tokens"], 1);
    assert_eq!(usage["usage"]["output_tokens"], 1);
}

#[tokio::test]
async fn exchange_hook_captures_complete_streaming_response_after_done() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("data:"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let payloads = captured.payloads.lock().unwrap();
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(exchange["completed"], true);
    assert_eq!(exchange["stream"], true);
    assert_eq!(
        exchange["response"]["body"]["choices"][0]["message"]["content"],
        "Hi"
    );
    let usage = payloads
        .iter()
        .find(|payload| payload.get("usage").is_some())
        .unwrap();
    assert_eq!(usage["usage"]["input_tokens"], 1);
    assert_eq!(usage["usage"]["output_tokens"], 1);
}

#[tokio::test]
async fn hooks_capture_reasoning_for_responses_stream_passthrough() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("response.reasoning_summary_text.delta"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let payloads = captured.payloads.lock().unwrap();
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(
        exchange["response"]["body"]["output"][0]["type"],
        "reasoning"
    );
    assert_eq!(
        exchange["response"]["body"]["output"][0]["summary"][0]["text"],
        "think"
    );

    let usage = payloads
        .iter()
        .find(|payload| payload.get("usage").is_some())
        .unwrap();
    assert_eq!(usage["usage"]["input_tokens"], 1);
    assert_eq!(usage["usage"]["output_tokens"], 2);
    assert_eq!(usage["usage"]["reasoning_tokens"], 1);
}

#[tokio::test]
async fn hooks_mark_cancelled_when_stream_is_dropped_early() {
    let (mock_base, _mock) = spawn_slow_openai_completion_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());

    let mut body_stream = res.bytes_stream();
    let first = body_stream.next().await.unwrap().unwrap();
    assert!(!first.is_empty());
    drop(body_stream);

    tokio::time::sleep(Duration::from_millis(250)).await;

    let payloads = captured.payloads.lock().unwrap();
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(exchange["completed"], false);
    assert_eq!(exchange["cancelled_by_client"], true);
    assert_eq!(exchange["partial"], true);
    assert_eq!(exchange["termination_reason"], "client_disconnected");

    let usage = payloads
        .iter()
        .find(|payload| payload.get("usage").is_some())
        .unwrap();
    assert_eq!(usage["status"], "cancelled");
    assert_eq!(usage["completed"], false);
    assert_eq!(usage["cancelled_by_client"], true);
    assert_eq!(usage["partial"], true);
    assert_eq!(usage["termination_reason"], "client_disconnected");
}

#[tokio::test]
async fn hooks_capture_translated_responses_reasoning_as_text_for_messages_stream() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{hook_base}/hook"),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gpt-4",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("event: error"), "body = {body}");
    assert!(
        body.contains("\"type\":\"invalid_request_error\""),
        "body = {body}"
    );
    assert!(body.contains("reasoning"), "body = {body}");
    assert!(body.contains("provenance"), "body = {body}");
    assert!(!body.contains("text_delta"), "body = {body}");
    assert!(!body.contains("message_stop"), "body = {body}");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let payloads = captured.payloads.lock().unwrap();
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(exchange["response"]["body"]["type"], "error");
    assert_eq!(
        exchange["response"]["body"]["error"]["type"],
        "invalid_request_error"
    );
    let message = exchange["response"]["body"]["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("reasoning"), "payload = {exchange:?}");
    assert!(message.contains("provenance"), "payload = {exchange:?}");
}

#[tokio::test]
async fn concurrent_openai_to_anthropic_requests_keep_headers_isolated_without_injecting_cache_control(
) {
    let (mock_base, _mock, captured) = spawn_concurrent_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let request_count = 24usize;
    let futures = (0..request_count).map(|i| {
        let client = client.clone();
        let proxy_base = proxy_base.clone();
        async move {
            client
                .post(format!("{proxy_base}/openai/v1/chat/completions"))
                .json(&json!({
                    "model": "gpt-4",
                    "messages": [
                        { "role": "system", "content": format!("System {i}") },
                        { "role": "user", "content": format!("Hello {i}") },
                        { "role": "assistant", "content": format!("Answer {i}") }
                    ]
                }))
                .send()
                .await
        }
    });

    let responses = join_all(futures).await;
    for res in responses {
        let res = res.unwrap();
        assert!(res.status().is_success(), "status: {}", res.status());
    }

    let requests = captured.requests.lock().unwrap();
    assert_eq!(requests.len(), request_count);

    for req in requests.iter() {
        let version = req
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
            .map(|(_, value)| value.as_str());
        assert_eq!(version, Some("2023-06-01"));

        assert_eq!(req.body["stream"], false);

        let system = req.body["system"]
            .as_array()
            .expect("system should be array");
        assert_eq!(system.len(), 1);
        assert!(system[0].get("cache_control").is_none());

        let messages = req.body["messages"]
            .as_array()
            .expect("messages should be array");
        assert_eq!(messages.len(), 2);

        let user_blocks = messages[0]["content"]
            .as_array()
            .expect("user content should be array");
        assert!(
            user_blocks
                .iter()
                .all(|block| block.get("cache_control").is_none()),
            "user blocks should not carry cache_control"
        );

        let assistant_blocks = messages[1]["content"]
            .as_array()
            .expect("assistant content should be array");
        assert!(assistant_blocks
            .iter()
            .all(|block| block.get("cache_control").is_none()));
    }
}

#[tokio::test]
async fn upstream_openai_completion_streaming_passthrough() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let text = res.text().await.unwrap();
    assert!(text.contains("data:"));
    assert!(text.contains("Hi") || text.contains("[DONE]"));
}

#[tokio::test]
async fn upstream_anthropic_streaming_translated_to_openai() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let text = res.text().await.unwrap();
    assert!(text.contains("data:"));
    assert!(
        text.contains("chat.completion.chunk") || text.contains("Hi") || text.contains("[DONE]")
    );
}

#[tokio::test]
async fn anthropic_messages_endpoint_streaming_translates_to_openai_upstream() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gpt-4",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("message_start"), "body = {body}");
    assert!(body.contains("message_stop"), "body = {body}");
}

#[tokio::test]
async fn responses_endpoint_streaming_translates_to_anthropic_upstream() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let body = res.text().await.unwrap();
    assert!(body.contains("response.completed"), "body = {body}");
    assert!(body.contains("\"Hi\""), "body = {body}");
}

#[tokio::test]
async fn responses_endpoint_streaming_preserves_plain_anthropic_thinking() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("event: response.reasoning_summary_text.delta"), "body = {body}");
    assert!(
        body.contains("\"delta\":\"think\""),
        "body = {body}"
    );
    assert!(body.contains("response.reasoning_summary_text.done"), "body = {body}");
    assert!(body.contains("response.completed"), "body = {body}");
    assert!(!body.contains("response.failed"), "body = {body}");
}

#[tokio::test]
async fn codex_minimax_anth_streaming_plain_thinking_succeeds() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Anthropic,
        "minimax-anth",
        "MiniMax-M2.7-highspeed",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "minimax-anth",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("reasoning_content"), "body = {body}");
    assert!(body.contains("\"content\":\"Hi\""), "body = {body}");
    assert!(!body.contains("\"finish_reason\":\"error\""), "body = {body}");
}

#[tokio::test]
async fn gemini_minimax_anth_streaming_plain_thinking_succeeds() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = config_with_alias(
        &mock_base,
        UpstreamFormat::Anthropic,
        "minimax-anth",
        "MiniMax-M2.7-highspeed",
    );
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/minimax-anth:streamGenerateContent"
        ))
        .json(&json!({
            "contents": [{ "role": "user", "parts": [{ "text": "Hi" }] }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("\"thought\":true"), "body = {body}");
    assert!(body.contains("\"text\":\"think\""), "body = {body}");
    assert!(body.contains("\"text\":\"Hi\""), "body = {body}");
}

#[tokio::test]
async fn debug_trace_records_request_delta_and_stream_summary() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let trace_path = std::env::temp_dir().join(format!(
        "llm-proxy-debug-trace-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    config.debug_trace = DebugTraceConfig {
        path: Some(trace_path.display().to_string()),
        max_text_chars: 256,
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "GLM-5",
            "input": "Hi",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("response.completed"), "body = {body}");

    tokio::time::sleep(Duration::from_millis(100)).await;
    let log = std::fs::read_to_string(&trace_path).unwrap();
    assert!(log.contains("\"phase\":\"request\""), "log = {log}");
    assert!(log.contains("\"phase\":\"response\""), "log = {log}");
    assert!(
        log.contains("\"new_items\":[{\"role\":\"user\",\"text\":\"Hi\",\"type\":\"message\"}]"),
        "log = {log}"
    );
    assert!(
        log.contains("\"terminal_event\":\"response.completed\""),
        "log = {log}"
    );
    assert!(log.contains("\"text\":\"Hi\""), "log = {log}");

    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test]
async fn debug_trace_records_google_stream_protocol_summary() {
    let (mock_base, _mock) = spawn_google_debug_trace_stream_mock().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::Google);
    let trace_path = std::env::temp_dir().join(format!(
        "llm-proxy-google-debug-trace-{}.jsonl",
        uuid::Uuid::new_v4()
    ));
    config.debug_trace = DebugTraceConfig {
        path: Some(trace_path.display().to_string()),
        max_text_chars: 256,
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-debug:streamGenerateContent"
        ))
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [{ "text": "Hi" }]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("functionCall"), "body = {body}");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let response_entry = loop {
        if let Ok(contents) = std::fs::read_to_string(&trace_path) {
            let parsed = contents
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|value| value.get("phase").and_then(Value::as_str) == Some("response"));
            if let Some(value) = parsed {
                break value;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for google debug trace response entry"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(response_entry["client_format"], "google");
    assert_eq!(response_entry["response"]["terminal_event"], "candidate");
    assert_eq!(response_entry["response"]["finish_reason"], "STOP");
    assert_eq!(response_entry["response"]["text"], "Hi");
    let tool_call = &response_entry["response"]["tool_calls"][0];
    let function_call = if tool_call.get("functionCall").is_some() {
        &tool_call["functionCall"]
    } else {
        tool_call
    };
    assert_eq!(function_call["id"], "call_1");
    assert_eq!(function_call["name"], "lookup_weather");
    assert_eq!(function_call["args"]["city"], "Tokyo");

    let _ = std::fs::remove_file(trace_path);
}

#[tokio::test]
async fn chat_completions_endpoint_preserves_responses_reasoning_stream() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("reasoning_content"), "body = {body}");
    assert!(body.contains("think"), "body = {body}");
    assert!(body.contains("\"finish_reason\":\"stop\""), "body = {body}");
}

#[tokio::test]
async fn messages_endpoint_preserves_responses_reasoning_stream() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gpt-4",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("event: error"), "body = {body}");
    assert!(
        body.contains("\"type\":\"invalid_request_error\""),
        "body = {body}"
    );
    assert!(body.contains("reasoning"), "body = {body}");
    assert!(body.contains("provenance"), "body = {body}");
    assert!(!body.contains("text_delta"), "body = {body}");
    assert!(!body.contains("message_stop"), "body = {body}");
}

#[tokio::test]
async fn upstream_google_client_openai_translated_non_streaming() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn upstream_google_client_openai_accepts_snake_case_input_parts() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/gemini-local:generateContent"
        ))
        .json(&json!({
            "model": "gemini-local",
            "contents": [{
                "parts": [{
                    "inline_data": {
                        "mime_type": "image/jpeg",
                        "data": "abcd"
                    }
                }]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["candidates"][0]["content"]["parts"][0]["text"], "Hi");
}

#[tokio::test]
async fn upstream_openai_responses_client_openai_completion_translated_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn upstream_openai_responses_streaming_passthrough() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "gpt-4",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Hi" }] }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.created") || text.contains("output_text") || text.contains("Hi")
    );
}

#[tokio::test]
async fn health_returns_ok() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/health"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// ---- Error and edge-case tests ----

#[tokio::test]
async fn post_invalid_json_returns_422_or_400() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_client_error(),
        "expected 4xx, got {}",
        res.status()
    );
}

#[tokio::test]
async fn post_empty_body_returns_4xx() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success() || res.status().is_client_error(),
        "got {}",
        res.status()
    );
}

#[tokio::test]
async fn upstream_unreachable_returns_502() {
    let config = Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_millis(100),
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: "http://127.0.0.1:31999/v1".to_string(),
            fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({ "model": "gpt-4", "messages": [{ "role": "user", "content": "Hi" }], "stream": false }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status().as_u16(),
        502,
        "expected 502 Bad Gateway when upstream unreachable"
    );
}

#[tokio::test]
async fn nonexistent_path_returns_404() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .get(format!("{proxy_base}/openai/v1/nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 404);
}

#[tokio::test]
async fn openai_completion_non_streaming_explicit_false() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let ct = res
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !ct.contains("event-stream"),
        "non-streaming must not return SSE"
    );
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn upstream_google_streaming_client_openai() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    assert_eq!(
        res.headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok()),
        Some("text/event-stream")
    );
    let text = res.text().await.unwrap();
    assert!(text.contains("data:"));
    assert!(
        text.contains("chat.completion.chunk") || text.contains("Hi") || text.contains("[DONE]")
    );
}
