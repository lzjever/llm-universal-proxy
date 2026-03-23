//! Full integration tests: proxy + mock upstreams per protocol.
//! Validates passthrough (same format) and translation (different format), non-streaming and streaming.

mod common;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use common::*;
use futures_util::{future::join_all, stream, StreamExt};
use llm_universal_proxy::config::{
    AuthPolicy, Config, HookConfig, HookEndpointConfig, ModelAlias, RuntimeConfigPayload,
    RuntimeHookConfig, RuntimeConfigSnapshot, RuntimeUpstreamConfig, UpstreamConfig,
};
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
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root: upstream_api_root(upstream_base, format),
            fixed_upstream_format: Some(format),
            fallback_credential_env: None,
            fallback_credential_actual: None,
            fallback_api_key: None,
            auth_policy: AuthPolicy::ClientOrFallback,
            upstream_headers: Vec::new(),
        }],
        model_aliases: Default::default(),
        hooks: Default::default(),
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
        fallback_credential_env: fallback_api_key.map(|_| format!("{}_KEY_ENV", name)),
        fallback_credential_actual: None,
        fallback_api_key: fallback_api_key.map(ToString::to_string),
        auth_policy: AuthPolicy::ClientOrFallback,
        upstream_headers: Vec::new(),
    }
}

fn upstream_api_root(upstream_base: &str, format: UpstreamFormat) -> String {
    let upstream_base = upstream_base.trim_end_matches('/');
    match format {
        UpstreamFormat::Google => format!("{}/v1beta", upstream_base),
        _ => format!("{}/v1", upstream_base),
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
async fn empty_startup_config_keeps_health_route_available() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let _proxy = tokio::spawn(async move { run_with_listener(Config::default(), listener).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = Client::new();
    let response = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

#[tokio::test]
async fn runtime_namespace_config_can_be_pushed_after_empty_start() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let apply = client
        .post(format!("{}/admin/namespaces/demo/config", proxy_base))
        .json(&RuntimeConfigSnapshot {
            revision: "rev-1".to_string(),
            config: RuntimeConfigPayload {
                listen: "127.0.0.1:0".to_string(),
                upstream_timeout_secs: 30,
                upstreams: vec![RuntimeUpstreamConfig {
                    name: "default".to_string(),
                    api_root: upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
                    fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                    fallback_credential_env: None,
                    fallback_credential_actual: None,
                    auth_policy: AuthPolicy::ClientOrFallback,
                    upstream_headers: Vec::new(),
                }],
                model_aliases: std::collections::BTreeMap::new(),
                hooks: RuntimeHookConfig::default(),
            },
        })
        .send()
        .await
        .unwrap();
    assert!(apply.status().is_success());

    let state: Value = client
        .get(format!("{}/admin/namespaces/demo/state", proxy_base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["revision"], "rev-1");
    assert_eq!(state["namespace"], "demo");

    let res = client
        .post(format!(
            "{}/namespaces/demo/openai/v1/chat/completions",
            proxy_base
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
async fn runtime_namespace_config_rejects_stale_or_duplicate_revision() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let (proxy_base, _proxy) = start_proxy(Config::default()).await;

    let client = Client::new();
    let payload = RuntimeConfigSnapshot {
        revision: "rev-2".to_string(),
        config: RuntimeConfigPayload {
            listen: "127.0.0.1:0".to_string(),
            upstream_timeout_secs: 30,
            upstreams: vec![RuntimeUpstreamConfig {
                name: "default".to_string(),
                api_root: upstream_api_root(&mock_base, UpstreamFormat::OpenAiCompletion),
                fixed_upstream_format: Some(UpstreamFormat::OpenAiCompletion),
                fallback_credential_env: None,
                fallback_credential_actual: None,
                auth_policy: AuthPolicy::ClientOrFallback,
                upstream_headers: Vec::new(),
            }],
            model_aliases: std::collections::BTreeMap::new(),
            hooks: RuntimeHookConfig::default(),
        },
    };

    let first = client
        .post(format!("{}/admin/namespaces/demo/config", proxy_base))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let duplicate = client
        .post(format!("{}/admin/namespaces/demo/config", proxy_base))
        .json(&payload)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let duplicate_body = duplicate.text().await.unwrap();
    assert!(duplicate_body.contains("stale or duplicate revision"));

    let stale = client
        .post(format!("{}/admin/namespaces/demo/config", proxy_base))
        .json(&RuntimeConfigSnapshot {
            revision: "rev-1".to_string(),
            config: payload.config,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn forwarded_headers_whitelist_preserves_protocol_headers_only() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);
    let captured = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let captured_clone = captured.clone();

    let app = Router::new().route(
        "/v1/messages",
        post(
            move |headers: HeaderMap, Json(body): Json<Value>| {
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
            },
        ),
    );
    let _mock = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let config = proxy_config(&base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;
    let client = Client::new();
    let response = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 77, 142, 93, 14, 130, 64, 12, 132, 239,
            50, 207, 96, 212, 199, 61, 129, 119, 48, 134, 172, 75, 133, 10, 108, 9, 173,
            137, 145, 112, 119, 139, 241, 239, 169, 201, 124, 51, 157, 153, 193, 53, 2,
            82, 27, 45, 13, 99, 95, 54, 15, 30, 81, 64, 206, 87, 74, 246, 6, 155, 36, 142,
            200, 88, 178, 163, 52, 81, 52, 242, 208, 174, 192, 32, 53, 245, 238, 90, 83,
            229, 32, 169, 91, 121, 43, 156, 72, 17, 142, 51, 56, 215, 116, 71, 216, 186,
            147, 84, 99, 67, 8, 51, 38, 233, 253, 34, 170, 178, 90, 204, 182, 102, 36, 27,
            229, 181, 239, 192, 88, 10, 92, 56, 179, 182, 149, 55, 169, 119, 6, 168, 201,
            136, 229, 84, 224, 246, 121, 50, 78, 190, 201, 42, 147, 142, 178, 190, 182,
            252, 70, 254, 171, 38, 22, 251, 175, 176, 95, 150, 39, 28, 44, 142, 26, 241,
            0, 0, 0,
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
    let mock_base = format!("http://127.0.0.1:{}", port);
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
async fn anthropic_namespace_messages_works() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
async fn anthropic_namespace_messages_stream_works() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
            "{}/google/v1beta/models/gemini-local:generateContent",
            proxy_base
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
            "{}/google/v1beta/models/gemini-local:streamGenerateContent",
            proxy_base
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
        .get(format!("{}/openai/v1/models", proxy_base))
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
        .get(format!("{}/anthropic/v1/models/haiku", proxy_base))
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
        .get(format!("{}/google/v1beta/models", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
            "{}/google/v1beta/models/gemini-1.5:generateContent",
            proxy_base
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
async fn upstream_openai_responses_passthrough_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
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
    let base = format!("http://127.0.0.1:{}", port);
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
    let base = format!("http://127.0.0.1:{}", port);
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

async fn spawn_hook_capture_server() -> (String, tokio::task::JoinHandle<()>, CapturedHookPayloads)
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{}", port);
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
    let base = format!("http://127.0.0.1:{}", port);
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
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
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
async fn hooks_capture_translated_thinking_blocks_for_messages_stream() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let (hook_base, _hook, captured) = spawn_hook_capture_server().await;
    let mut config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    config.hooks = HookConfig {
        max_pending_bytes: 100 * 1024 * 1024,
        timeout: Duration::from_secs(3),
        failure_threshold: 3,
        cooldown: Duration::from_secs(300),
        exchange: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
        usage: Some(HookEndpointConfig {
            url: format!("{}/hook", hook_base),
            authorization: None,
        }),
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
    assert!(body.contains("thinking_delta"));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let payloads = captured.payloads.lock().unwrap();
    let exchange = payloads
        .iter()
        .find(|payload| payload.get("request").is_some())
        .unwrap();
    assert_eq!(
        exchange["response"]["body"]["content"][0]["type"],
        "thinking"
    );
    assert_eq!(
        exchange["response"]["body"]["content"][0]["thinking"],
        "think"
    );
    assert_eq!(exchange["response"]["body"]["content"][1]["type"], "text");
    assert_eq!(exchange["response"]["body"]["content"][1]["text"], "Hi");
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
                .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        let last = assistant_blocks
            .last()
            .expect("assistant block should exist");
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
async fn responses_endpoint_streaming_preserves_anthropic_reasoning() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
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
    assert!(
        body.contains("response.reasoning_summary_text.delta"),
        "body = {body}"
    );
    assert!(
        body.contains("response.reasoning_summary_text.done"),
        "body = {body}"
    );
    assert!(body.contains("\"think\""), "body = {body}");
    assert!(body.contains("response.completed"), "body = {body}");
}

#[tokio::test]
async fn chat_completions_endpoint_preserves_responses_reasoning_stream() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/anthropic/v1/messages", proxy_base))
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
    assert!(
        body.contains("\"type\":\"thinking\"") || body.contains("\"type\": \"thinking\""),
        "body = {body}"
    );
    assert!(body.contains("thinking_delta"), "body = {body}");
    assert!(body.contains("message_stop"), "body = {body}");
}

#[tokio::test]
async fn upstream_google_client_openai_translated_non_streaming() {
    let (mock_base, _mock) = spawn_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/responses", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    };
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .get(format!("{}/openai/v1/nonexistent", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
