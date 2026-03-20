//! Full integration tests: proxy + mock upstreams per protocol.
//! Validates passthrough (same format) and translation (different format), non-streaming and streaming.

mod common;

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use common::*;
use futures_util::future::join_all;
use llm_universal_proxy::config::Config;
use llm_universal_proxy::formats::UpstreamFormat;
use llm_universal_proxy::server::run_with_listener;
use reqwest::Client;
use serde_json::json;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

fn proxy_config(upstream_base: &str, format: UpstreamFormat) -> Config {
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_url: upstream_base.to_string(),
        fixed_upstream_format: Some(format),
        upstream_timeout: Duration::from_secs(30),
        upstream_api_key: None,
        upstream_headers: Vec::new(),
    }
}

/// Start proxy with config; returns (proxy_base_url, _handle).
async fn start_proxy(
    config: Config,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);
    let handle = tokio::spawn(async move { run_with_listener(config, listener).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, handle)
}

#[tokio::test]
async fn upstream_openai_completion_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
    // Response is translated to Anthropic format (content array).
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
async fn upstream_google_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
        .json(&json!({
            "contents": [{ "parts": [{ "text": "Hi" }] }],
            "model": "gemini-1.5"
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
async fn upstream_openai_responses_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/responses", proxy_base))
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

async fn spawn_header_capture_anthropic_mock(
) -> (String, tokio::task::JoinHandle<()>, CapturedHeaders) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);
    let state = CapturedHeaders::default();
    let app = Router::new()
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

async fn spawn_concurrent_capture_anthropic_mock(
) -> (String, tokio::task::JoinHandle<()>, CapturedAnthropicRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);
    let state = CapturedAnthropicRequests::default();
    let app = Router::new()
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
    state.requests.lock().unwrap().push(CapturedAnthropicRequest {
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

#[tokio::test]
async fn upstream_anthropic_injects_required_version_header() {
    let (mock_base, _mock, captured) = spawn_header_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
async fn concurrent_openai_to_anthropic_requests_keep_headers_and_cache_control_isolated() {
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
                .post(format!("{}/v1/chat/completions", proxy_base))
                .json(&json!({
                    "model": "gpt-4",
                    "messages": [
                        { "role": "system", "content": format!("System {}", i) },
                        { "role": "user", "content": format!("Hello {}", i) },
                        { "role": "assistant", "content": format!("Answer {}", i) }
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
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(system[0]["cache_control"]["ttl"], "1h");

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
        let last = assistant_blocks.last().expect("assistant block should exist");
        assert_eq!(last["cache_control"]["type"], "ephemeral");
        assert!(
            assistant_blocks[..assistant_blocks.len() - 1]
                .iter()
                .all(|block| block.get("cache_control").is_none()),
            "only last assistant block should carry cache_control"
        );
    }
}

#[tokio::test]
async fn upstream_openai_completion_streaming_passthrough() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
async fn upstream_google_client_openai_translated_non_streaming() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
async fn upstream_openai_responses_client_openai_completion_translated_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/responses", proxy_base))
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
        .get(format!("{}/health", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        upstream_url: "http://127.0.0.1:31999".to_string(),
        fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
        upstream_timeout: Duration::from_millis(100),
        upstream_api_key: None,
        upstream_headers: Vec::new(),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .get(format!("{}/v1/nonexistent", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
        .post(format!("{}/v1/chat/completions", proxy_base))
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
