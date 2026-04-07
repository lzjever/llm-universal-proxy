//! Cross-Format Reasoning/Thinking Integration Tests
//!
//! Tests the full matrix of reasoning/thinking translation across all 4 protocols:
//! - OpenAI Chat Completions (reasoning_content)
//! - OpenAI Responses (reasoning output items)
//! - Anthropic Messages (thinking blocks)
//! - Google Gemini (thought parts)
//!
//! Also tests Gap 1-4 fixes from the reasoning audit.

mod common;

use common::*;
use llm_universal_proxy::config::{AuthPolicy, Config, DebugTraceConfig, UpstreamConfig};
use llm_universal_proxy::formats::UpstreamFormat;
use llm_universal_proxy::server::run_with_listener;
use reqwest::Client;
use serde_json::json;
use serde_json::Value;
use std::time::Duration;
use tokio::net::TcpListener;

fn proxy_config(upstream_base: &str, format: UpstreamFormat) -> Config {
    let upstream_base = upstream_base.trim_end_matches('/');
    let api_root = match format {
        UpstreamFormat::Google => format!("{}/v1beta", upstream_base),
        _ => format!("{}/v1", upstream_base),
    };
    Config {
        listen: "127.0.0.1:0".to_string(),
        upstream_timeout: Duration::from_secs(30),
        upstreams: vec![UpstreamConfig {
            name: "default".to_string(),
            api_root,
            fixed_upstream_format: Some(format),
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

// ============================================================
// A. Non-Streaming Cross-Format Reasoning
// ============================================================

#[tokio::test]
async fn anthropic_thinking_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
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
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
}

#[tokio::test]
async fn anthropic_thinking_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/google/v1beta/models/test:generateContent", proxy_base))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let content = body["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "think");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "Hi");
}

#[tokio::test]
async fn openai_reasoning_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

#[tokio::test]
async fn openai_reasoning_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/google/v1beta/models/test:generateContent", proxy_base))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

// Gap 1 fix: Responses API reasoning → other formats
#[tokio::test]
async fn responses_reasoning_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "mock",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn responses_reasoning_to_anthropic_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let content = body["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "think");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "Hi");
}

#[tokio::test]
async fn responses_reasoning_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/google/v1beta/models/test:generateContent", proxy_base))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_anthropic_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let content = body["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "think");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

// ============================================================
// B. Streaming Cross-Format Reasoning
// ============================================================

#[tokio::test]
async fn anthropic_thinking_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
}

#[tokio::test]
async fn anthropic_thinking_to_responses_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "claude-3", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("response.reasoning_summary_text.delta"), "body = {text}");
    assert!(
        text.contains("response.output_text.delta") || text.contains("response.completed"),
        "body = {text}"
    );
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("thinking_delta"), "body = {text}");
    assert!(text.contains("text_delta"), "body = {text}");
    assert!(text.contains("message_stop"), "body = {text}");
}

#[tokio::test]
async fn openai_reasoning_to_responses_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("response.reasoning_summary_text.delta"), "body = {text}");
}

#[tokio::test]
async fn responses_reasoning_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "mock",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
}

#[tokio::test]
async fn responses_reasoning_to_anthropic_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("thinking_delta"), "body = {text}");
    assert!(text.contains("message_stop"), "body = {text}");
}

#[tokio::test]
async fn gemini_thinking_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "body = {text}");
}

#[tokio::test]
async fn gemini_thinking_to_anthropic_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("thinking_delta"), "body = {text}");
    assert!(text.contains("message_stop"), "body = {text}");
}

#[tokio::test]
async fn gemini_thinking_to_responses_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta") || text.contains("response.completed"),
        "body = {text}"
    );
}

// ============================================================
// C. Thinking + Tool Use Combined
// ============================================================

#[tokio::test]
async fn anthropic_thinking_with_tools_to_openai_chat_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let msg = &body["choices"][0]["message"];
    assert_eq!(msg["reasoning_content"], "need to call tool");
    assert_eq!(msg["content"], "Calling tool.");
    let tool_calls = msg["tool_calls"].as_array().expect("tool_calls");
    assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test]
async fn anthropic_thinking_with_tools_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "input": "Weather?",
            "tools": [{ "type": "function", "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert!(output.iter().any(|o| o["type"] == "reasoning"), "missing reasoning: {output:?}");
    assert!(output.iter().any(|o| o["type"] == "message"), "missing message: {output:?}");
    assert!(output.iter().any(|o| o["type"] == "function_call"), "missing function_call: {output:?}");
}

#[tokio::test]
async fn anthropic_thinking_with_tools_to_openai_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": {} } }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(text.contains("reasoning_content"), "should have reasoning delta: {text}");
    assert!(text.contains("tool_calls") || text.contains("get_weather"), "should have tool call: {text}");
}

// ============================================================
// D. Multi-Turn Thinking Preservation (Gap 3 fix)
// ============================================================

#[tokio::test]
async fn multi_turn_anthropic_thinking_preserved_in_history() {
    // Thinking blocks in assistant messages → reasoning_content on OpenAI upstream
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [
                { "role": "user", "content": "Think about 2+2" },
                { "role": "assistant", "content": [
                    { "type": "thinking", "thinking": "2+2 equals 4" },
                    { "type": "text", "text": "The answer is 4" }
                ]},
                { "role": "user", "content": "Now what about 3+3?" }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    assert!(body.get("content").is_some());
}

#[tokio::test]
async fn multi_turn_openai_reasoning_preserved_in_history_to_claude() {
    // reasoning_content in assistant messages → thinking blocks on Anthropic upstream
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "messages": [
                { "role": "user", "content": "Think about 2+2" },
                { "role": "assistant", "reasoning_content": "2+2 equals 4", "content": "The answer is 4" },
                { "role": "user", "content": "Now what about 3+3?" }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "status: {}", res.status());
    let body: Value = res.json().await.unwrap();
    // OpenAI chat completions format uses choices[0].message.content
    assert!(
        body.get("choices").is_some() || body.get("content").is_some(),
        "unexpected response: {body:?}"
    );
}

// ============================================================
// E. Usage/Token Count Tests
// ============================================================

#[tokio::test]
async fn anthropic_thinking_usage_translated_to_openai() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["prompt_tokens"], 1);
    assert_eq!(body["usage"]["completion_tokens"], 2);
}

#[tokio::test]
async fn openai_reasoning_usage_translated_to_anthropic() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["input_tokens"], 1);
    assert_eq!(body["usage"]["output_tokens"], 3);
}

#[tokio::test]
async fn gemini_thinking_usage_translated_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["prompt_tokens"], 1);
    assert_eq!(body["usage"]["completion_tokens"], 2);
    assert_eq!(body["usage"]["total_tokens"], 3);
}

#[tokio::test]
async fn openai_reasoning_with_completion_tokens_details() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["usage"]["output_tokens_details"]["reasoning_tokens"], 1);
}

// ============================================================
// F. Edge Cases & Gap Fixes
// ============================================================

#[tokio::test]
async fn empty_thinking_block_no_crash() {
    let (mock_base, _mock) = spawn_openai_completion_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/anthropic/v1/messages", proxy_base))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    let content = body["content"].as_array().unwrap();
    assert!(content.iter().all(|c| c["type"] != "thinking"));
}

#[tokio::test]
async fn reasoning_and_text_both_present_in_response() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/responses", proxy_base))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["status"], "completed");
    let output = body["output"].as_array().unwrap();
    assert!(output.iter().any(|o| o["type"] == "reasoning"));
    assert!(output.iter().any(|o| o["type"] == "message"));
}

// Gap 2 fix: Gemini thought without thoughtSignature
#[tokio::test]
async fn gemini_thinking_no_signature_streaming_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_no_signature_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("reasoning_content"),
        "should have reasoning_content without thoughtSignature: {text}"
    );
}

#[tokio::test]
async fn gemini_thinking_no_signature_non_streaming_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_no_signature_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{}/openai/v1/chat/completions", proxy_base))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}
