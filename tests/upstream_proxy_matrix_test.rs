#[path = "common/forward_proxy.rs"]
mod forward_proxy;
#[path = "common/runtime_proxy.rs"]
mod runtime_proxy;

use axum::{
    body::Body,
    extract::{Json, Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json as AxumJson, Router,
};
use forward_proxy::spawn_http_forward_proxy;
use llm_universal_proxy::config::{
    CompatibilityMode, Config, DebugTraceConfig, RuntimeConfigPayload, UpstreamConfig,
};
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::{
    header::{HeaderMap as ReqwestHeaderMap, HeaderValue},
    Client,
};
use runtime_proxy::{start_proxy, upstream_api_root};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

static UPSTREAM_PROXY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
const TEST_PROVIDER_KEY: &str = "provider-secret";

fn direct_data_client() -> Client {
    let mut headers = ReqwestHeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TEST_PROVIDER_KEY}")).unwrap(),
    );
    Client::builder()
        .no_proxy()
        .default_headers(headers)
        .build()
        .unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedUpstreamRequest {
    method: String,
    path: String,
    body: Option<Value>,
}

#[derive(Clone, Default)]
struct CapturedUpstreamRequests {
    requests: Arc<Mutex<Vec<CapturedUpstreamRequest>>>,
}

impl CapturedUpstreamRequests {
    fn push(&self, request: CapturedUpstreamRequest) {
        self.requests.lock().unwrap().push(request);
    }

    fn snapshot(&self) -> Vec<CapturedUpstreamRequest> {
        self.requests.lock().unwrap().clone()
    }
}

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

fn openai_auto_discovery_config(upstream_base: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        compatibility_mode: CompatibilityMode::Balanced,
        proxy: None,
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, UpstreamFormat::OpenAiCompletion),
            fixed_upstream_format: None,
            provider_key_env: None,
            provider_key: None,
            upstream_headers: Vec::new(),
            proxy: None,
            limits: None,
            surface_defaults: None,
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
        debug_trace: DebugTraceConfig::default(),
        resource_limits: Default::default(),
        conversation_state_bridge: Default::default(),
        data_auth: None,
    }
}

async fn spawn_openai_capture_upstream() -> (
    String,
    tokio::task::JoinHandle<()>,
    CapturedUpstreamRequests,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let captured = CapturedUpstreamRequests::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(openai_chat_handler))
        .route("/v1/responses", post(openai_responses_create_handler))
        .route("/v1/responses/:id", get(openai_responses_get_handler))
        .with_state(captured.clone());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, handle, captured)
}

async fn openai_chat_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    capture_request(
        &captured,
        method,
        "/v1/chat/completions",
        Some(body.clone()),
    );
    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if stream {
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .body(Body::from("data: [DONE]\n\n"))
            .unwrap();
    }
    let mut response = (
        StatusCode::OK,
        AxumJson(json!({
            "id": "chatcmpl-proxy-test",
            "object": "chat.completion",
            "created": 1,
            "model": body.get("model").cloned().unwrap_or_else(|| json!("mock")),
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })),
    )
        .into_response();
    if let Some(value) = headers.get("x-proxy-test-id") {
        response
            .headers_mut()
            .insert("x-proxy-test-id", value.clone());
    }
    response
}

async fn openai_responses_create_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    Json(body): Json<Value>,
) -> Response {
    capture_request(&captured, method, "/v1/responses", Some(body.clone()));
    (
        StatusCode::OK,
        AxumJson(json!({
            "id": "resp_proxy_create",
            "object": "response",
            "model": body.get("model").cloned().unwrap_or_else(|| json!("gpt-4o")),
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "hi"
                }]
            }]
        })),
    )
        .into_response()
}

async fn openai_responses_get_handler(
    State(captured): State<CapturedUpstreamRequests>,
    method: Method,
    Path(id): Path<String>,
) -> Response {
    capture_request(&captured, method, &format!("/v1/responses/{id}"), None);
    (
        StatusCode::OK,
        AxumJson(json!({
            "id": id,
            "object": "response",
            "model": "gpt-4o",
            "output": [{
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "resource ok"
                }]
            }]
        })),
    )
        .into_response()
}

fn capture_request(
    captured: &CapturedUpstreamRequests,
    method: Method,
    path: &str,
    body: Option<Value>,
) {
    captured.push(CapturedUpstreamRequest {
        method: method.to_string(),
        path: path.to_string(),
        body,
    });
}

async fn wait_for_upstream_path(
    captured: &CapturedUpstreamRequests,
    path: &str,
    attempts: usize,
) -> Vec<CapturedUpstreamRequest> {
    for _ in 0..attempts {
        let snapshot = captured.snapshot();
        if snapshot.iter().any(|request| request.path == path) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    captured.snapshot()
}

async fn wait_for_upstream_request_count(
    captured: &CapturedUpstreamRequests,
    minimum: usize,
    attempts: usize,
) -> Vec<CapturedUpstreamRequest> {
    for _ in 0..attempts {
        let snapshot = captured.snapshot();
        if snapshot.len() >= minimum {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    captured.snapshot()
}

#[test]
fn yaml_proxy_config_round_trip_preserves_namespace_and_per_upstream_override_layers() {
    let config = Config::from_yaml_str(
        r#"
listen: 127.0.0.1:0
proxy: direct
upstreams:
  OPENAI:
    api_root: http://example.com/v1
    format: openai-completion
    proxy:
      url: http://upstream-proxy.local:8080
"#,
    )
    .unwrap();

    let round_trip = serde_json::to_value(RuntimeConfigPayload::from(&config)).unwrap();

    assert_eq!(round_trip["proxy"], "direct");
    assert_eq!(
        round_trip["upstreams"][0]["proxy"]["url"],
        "http://upstream-proxy.local:8080"
    );
}

#[tokio::test]
async fn env_proxy_is_used_consistently_for_discovery_request_and_resource_paths() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (proxy_base, _forward_proxy, captured_proxy) = spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &proxy_base);
    let _all_proxy = ScopedEnvVar::remove("ALL_PROXY");
    let _all_proxy_lower = ScopedEnvVar::remove("all_proxy");
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let config = openai_auto_discovery_config(&upstream_base);
    let (llmup_base, _proxy_handle) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let resource = client
        .get(format!("{llmup_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resource.status(), StatusCode::OK);

    let proxied = captured_proxy
        .wait_for_count(3, Duration::from_secs(2))
        .await;
    let joined_uris = proxied
        .iter()
        .map(|item| item.uri.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined_uris.contains("/v1/chat/completions"),
        "proxy did not observe request path traffic: {joined_uris}"
    );
    assert!(
        joined_uris.contains("/v1/responses"),
        "proxy did not observe discovery or resource traffic: {joined_uris}"
    );
    assert!(
        joined_uris.contains("/v1/responses/resp_123"),
        "proxy did not observe resource path traffic: {joined_uris}"
    );

    let upstream_requests =
        wait_for_upstream_path(&captured_upstream, "/v1/responses/resp_123", 80).await;
    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/chat/completions"),
        "upstream did not receive request path traffic: {upstream_requests:?}"
    );
    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/responses/resp_123"),
        "upstream did not receive resource path traffic: {upstream_requests:?}"
    );
    assert!(
        upstream_requests.iter().any(|request| {
            request.path == "/v1/responses"
                && request
                    .body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .is_some()
        }),
        "upstream did not receive discovery probe for responses: {upstream_requests:?}"
    );
}

#[tokio::test]
async fn per_upstream_override_should_override_namespace_and_env_for_request_path() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, _captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (namespace_proxy_base, _namespace_proxy, captured_namespace_proxy) =
        spawn_http_forward_proxy().await;
    let (override_proxy_base, _override_proxy, captured_override_proxy) =
        spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy:
  url: {namespace_proxy_base}
upstreams:
  OPENAI:
    api_root: {api_root}
    format: openai-completion
    proxy:
      url: {override_proxy_base}
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let override_seen = captured_override_proxy
        .wait_for_count(1, Duration::from_secs(1))
        .await;
    let namespace_seen = captured_namespace_proxy.snapshot();
    let env_seen = captured_env_proxy.snapshot();

    assert_eq!(override_seen.len(), 1, "per-upstream override should win");
    assert!(
        namespace_seen.is_empty(),
        "namespace proxy should be shadowed by upstream proxy: {namespace_seen:?}"
    );
    assert!(
        env_seen.is_empty(),
        "env proxy should be shadowed by upstream proxy: {env_seen:?}"
    );
}

#[tokio::test]
async fn explicit_direct_should_cut_env_proxy() {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy: direct
upstreams:
  OPENAI:
    api_root: {api_root}
    format: openai-completion
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let upstream_requests =
        wait_for_upstream_path(&captured_upstream, "/v1/chat/completions", 80).await;
    let env_requests = captured_env_proxy.snapshot();

    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/chat/completions"),
        "direct mode should still reach the upstream: {upstream_requests:?}"
    );
    assert!(
        env_requests.is_empty(),
        "direct mode should bypass env proxy entirely: {env_requests:?}"
    );
}

#[tokio::test]
async fn per_upstream_direct_should_bypass_namespace_and_env_for_request_resource_and_streaming_paths(
) {
    let _env_lock = UPSTREAM_PROXY_ENV_LOCK.lock().await;
    let (upstream_base, _upstream, captured_upstream) = spawn_openai_capture_upstream().await;
    let (env_proxy_base, _env_proxy, captured_env_proxy) = spawn_http_forward_proxy().await;
    let (namespace_proxy_base, _namespace_proxy, captured_namespace_proxy) =
        spawn_http_forward_proxy().await;
    let _http_proxy = ScopedEnvVar::set("HTTP_PROXY", &env_proxy_base);
    let _http_proxy_lower = ScopedEnvVar::set("http_proxy", &env_proxy_base);
    let _no_proxy = ScopedEnvVar::remove("NO_PROXY");
    let _no_proxy_lower = ScopedEnvVar::remove("no_proxy");
    let _request_method = ScopedEnvVar::remove("REQUEST_METHOD");

    let yaml = format!(
        r#"
listen: 127.0.0.1:0
proxy:
  url: {namespace_proxy_base}
upstreams:
  OPENAI:
    api_root: {api_root}
    proxy: direct
"#,
        api_root = upstream_api_root(&upstream_base, UpstreamFormat::OpenAiCompletion),
    );
    let config = Config::from_yaml_str(&yaml).unwrap();
    let (llmup_base, _llmup) = start_proxy(config).await;
    let client = direct_data_client();

    let request_response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(request_response.status(), StatusCode::OK);

    let resource_response = client
        .get(format!("{llmup_base}/openai/v1/responses/resp_123"))
        .send()
        .await
        .unwrap();
    assert_eq!(resource_response.status(), StatusCode::OK);

    let streaming_response = client
        .post(format!("{llmup_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4o",
            "stream": true,
            "messages": [{ "role": "user", "content": "stream ping" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(streaming_response.status(), StatusCode::OK);
    assert_eq!(
        streaming_response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let streaming_body = streaming_response.text().await.unwrap();
    assert!(streaming_body.contains("[DONE]"));

    let upstream_requests = wait_for_upstream_request_count(&captured_upstream, 4, 80).await;
    let chat_requests = upstream_requests
        .iter()
        .filter(|request| request.path == "/v1/chat/completions")
        .collect::<Vec<_>>();

    assert!(
        upstream_requests
            .iter()
            .any(|request| request.path == "/v1/responses/resp_123"),
        "resource path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        upstream_requests.iter().any(|request| {
            request.path == "/v1/responses"
                && request
                    .body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .is_some()
        }),
        "resource discovery should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.len() >= 2,
        "request and streaming paths should both reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.iter().any(|request| {
            request
                .body
                .as_ref()
                .and_then(|body| body.get("stream"))
                .and_then(Value::as_bool)
                == Some(true)
        }),
        "streaming path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        chat_requests.iter().any(|request| {
            request
                .body
                .as_ref()
                .and_then(|body| body.get("stream"))
                .and_then(Value::as_bool)
                != Some(true)
        }),
        "request path should reach upstream directly: {upstream_requests:?}"
    );
    assert!(
        captured_namespace_proxy.snapshot().is_empty(),
        "namespace proxy should be bypassed by per-upstream direct: {:?}",
        captured_namespace_proxy.snapshot()
    );
    assert!(
        captured_env_proxy.snapshot().is_empty(),
        "env proxy should be bypassed by per-upstream direct: {:?}",
        captured_env_proxy.snapshot()
    );
}
