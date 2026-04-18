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
use llm_universal_proxy::formats::UpstreamFormat;
use reqwest::Client;
use serde_json::json;
use serde_json::Value;

async fn assert_reasoning_to_anthropic_rejected(res: reqwest::Response) {
    let status = res.status();
    let body: Value = res.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "body = {body:?}");
    if let Some(body_type) = body.get("type").and_then(Value::as_str) {
        assert_eq!(body_type, "error", "body = {body:?}");
    }
    assert_eq!(
        body["error"]["type"], "invalid_request_error",
        "body = {body:?}"
    );
    let message = body["error"]["message"]
        .as_str()
        .expect("anthropic error message");
    assert!(message.contains("reasoning"), "body = {body:?}");
    assert!(message.contains("provenance"), "body = {body:?}");
}

async fn assert_reasoning_to_anthropic_stream_rejected(res: reqwest::Response) {
    let status = res.status();
    let body = res.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body = {body}");
    assert!(body.contains("event: error"), "body = {body}");
    assert!(body.contains("\"type\":\"error\""), "body = {body}");
    assert!(
        body.contains("\"type\":\"invalid_request_error\""),
        "body = {body}"
    );
    assert!(body.contains("reasoning"), "body = {body}");
    assert!(body.contains("provenance"), "body = {body}");
    assert!(!body.contains("text_delta"), "body = {body}");
    assert!(!body.contains("message_stop"), "body = {body}");
}

async fn assert_anthropic_thinking_to_openai_stream_failed_closed(
    res: reqwest::Response,
) -> String {
    let status = res.status();
    let body = res.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body = {body}");
    assert!(
        body.contains("\"finish_reason\":\"error\""),
        "body = {body}"
    );
    assert!(
        body.contains("\"type\":\"invalid_request_error\""),
        "body = {body}"
    );
    assert!(
        body.contains("\"code\":\"unsupported_anthropic_stream_event\""),
        "body = {body}"
    );
    assert!(
        body.contains("Anthropic thinking blocks cannot be translated losslessly."),
        "body = {body}"
    );
    assert!(!body.contains("reasoning_content"), "body = {body}");
    assert!(!body.contains("<think>"), "body = {body}");
    body
}

async fn assert_anthropic_thinking_to_responses_stream_failed_closed(
    res: reqwest::Response,
) -> String {
    let status = res.status();
    let body = res.text().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::OK, "body = {body}");
    assert!(body.contains("event: response.failed"), "body = {body}");
    assert!(
        body.contains("\"type\":\"invalid_request_error\""),
        "body = {body}"
    );
    assert!(
        body.contains("\"code\":\"unsupported_anthropic_stream_event\""),
        "body = {body}"
    );
    assert!(
        body.contains("Anthropic thinking blocks cannot be translated losslessly."),
        "body = {body}"
    );
    assert!(
        !body.contains("response.reasoning_summary_text.delta"),
        "body = {body}"
    );
    assert!(
        !body.contains("response.output_text.delta"),
        "body = {body}"
    );
    assert!(!body.contains("response.completed"), "body = {body}");
    assert!(!body.contains("<think>"), "body = {body}");
    body
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
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
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["thought"], true);
    assert_eq!(parts[0]["text"], "think");
    assert_eq!(parts[1]["text"], "Hi");
}

#[tokio::test]
async fn anthropic_thinking_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "think");
    assert!(output.iter().any(|o| o["type"] == "message"));
}

#[tokio::test]
async fn anthropic_signed_thinking_to_responses_non_streaming_returns_carrier() {
    let (mock_base, _mock) = spawn_anthropic_signed_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "internal reasoning");
    assert!(
        output[0]["encrypted_content"].is_string(),
        "body = {body:?}"
    );
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "Visible answer");
}

#[tokio::test]
async fn anthropic_signed_thinking_responses_round_trip_non_streaming_replays_carrier_to_upstream()
{
    let (source_base, _source_mock) = spawn_anthropic_signed_thinking_mock().await;
    let source_config = proxy_config(&source_base, UpstreamFormat::Anthropic);
    let (source_proxy_base, _source_proxy) = start_proxy(source_config).await;

    let client = Client::new();
    let first_response = client
        .post(format!("{source_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(first_response.status().is_success());
    let first_body: Value = first_response.json().await.unwrap();
    let output = first_body["output"].as_array().expect("responses output");
    let reasoning_item = output[0].clone();
    let message_item = output[1].clone();

    let (capture_base, _capture_mock, mut captured) = spawn_capture_anthropic_mock().await;
    let capture_config = proxy_config(&capture_base, UpstreamFormat::Anthropic);
    let (capture_proxy_base, _capture_proxy) = start_proxy(capture_config).await;

    let second_response = client
        .post(format!("{capture_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Think about it" }]
                },
                reasoning_item,
                message_item,
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Continue" }]
                }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        second_response.status().is_success(),
        "status: {status}",
        status = second_response.status()
    );
    let _: Value = second_response.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["thinking"], "internal reasoning");
    assert_eq!(assistant_content[0]["signature"], "sig_123");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Continue");
}

#[tokio::test]
async fn anthropic_omitted_thinking_responses_round_trip_non_streaming_replays_carrier_to_upstream()
{
    let (source_base, _source_mock) = spawn_anthropic_omitted_thinking_mock().await;
    let source_config = proxy_config(&source_base, UpstreamFormat::Anthropic);
    let (source_proxy_base, _source_proxy) = start_proxy(source_config).await;

    let client = Client::new();
    let first_response = client
        .post(format!("{source_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Hi",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(first_response.status().is_success());
    let first_body: Value = first_response.json().await.unwrap();
    let output = first_body["output"].as_array().expect("responses output");
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"], json!([]));
    assert!(
        output[0]["encrypted_content"].is_string(),
        "body = {first_body:?}"
    );
    let reasoning_item = output[0].clone();
    let message_item = output[1].clone();

    let (capture_base, _capture_mock, mut captured) = spawn_capture_anthropic_mock().await;
    let capture_config = proxy_config(&capture_base, UpstreamFormat::Anthropic);
    let (capture_proxy_base, _capture_proxy) = start_proxy(capture_config).await;

    let second_response = client
        .post(format!("{capture_proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Think about it" }]
                },
                reasoning_item,
                message_item,
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "Continue" }]
                }
            ],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        second_response.status().is_success(),
        "status: {status}",
        status = second_response.status()
    );
    let _: Value = second_response.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured
        .borrow()
        .clone()
        .expect("captured anthropic request");
    let messages = request["messages"].as_array().expect("anthropic messages");
    assert_eq!(messages[1]["role"], "assistant");
    let assistant_content = messages[1]["content"]
        .as_array()
        .expect("assistant content");
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(
        assistant_content[0]["thinking"],
        json!({ "display": "omitted" })
    );
    assert_eq!(assistant_content[0]["signature"], "sig_omitted");
    assert_eq!(assistant_content[1]["type"], "text");
    assert_eq!(assistant_content[1]["text"], "Visible answer");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["text"], "Continue");
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_non_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_rejected(res).await;
}

#[tokio::test]
async fn openai_reasoning_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
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
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "mock",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn responses_reasoning_to_anthropic_non_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_rejected(res).await;
}

#[tokio::test]
async fn responses_reasoning_to_gemini_non_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!(
            "{proxy_base}/google/v1beta/models/test:generateContent"
        ))
        .json(&json!({ "contents": [{ "parts": [{ "text": "Hi" }] }] }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["reasoning_content"], "think");
    assert_eq!(body["choices"][0]["message"]["content"], "Hi");
}

#[tokio::test]
async fn gemini_thinking_to_anthropic_non_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_rejected(res).await;
}

#[tokio::test]
async fn gemini_thinking_to_responses_non_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
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
async fn anthropic_thinking_to_openai_chat_streaming_fails_closed() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_anthropic_thinking_to_openai_stream_failed_closed(res).await;
}

#[tokio::test]
async fn anthropic_thinking_to_responses_streaming_fails_closed() {
    let (mock_base, _mock) = spawn_anthropic_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "claude-3", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert_anthropic_thinking_to_responses_stream_failed_closed(res).await;
}

#[tokio::test]
async fn openai_reasoning_to_anthropic_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_rejected(res).await;
}

#[tokio::test]
async fn openai_reasoning_to_responses_streaming() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta"),
        "body = {text}"
    );
}

#[tokio::test]
async fn responses_reasoning_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
async fn responses_reasoning_to_anthropic_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_openai_responses_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiResponses);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_rejected(res).await;
}

#[tokio::test]
async fn gemini_thinking_to_openai_chat_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
async fn gemini_thinking_to_anthropic_streaming_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "gemini-test",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_stream_rejected(res).await;
}

#[tokio::test]
async fn gemini_thinking_to_responses_streaming() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "gemini-test", "input": "Hi", "stream": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let text = res.text().await.unwrap();
    assert!(
        text.contains("response.reasoning_summary_text.delta")
            || text.contains("response.completed"),
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
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
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({
            "model": "claude-3",
            "input": "Weather?",
            "tools": [{ "type": "function", "name": "get_weather", "parameters": { "type": "object", "properties": { "city": { "type": "string" } } } }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    let output = body["output"].as_array().unwrap();
    assert!(
        output.iter().any(|o| o["type"] == "reasoning"),
        "missing reasoning: {output:?}"
    );
    assert!(
        output.iter().any(|o| o["type"] == "message"),
        "missing message: {output:?}"
    );
    assert!(
        output.iter().any(|o| o["type"] == "function_call"),
        "missing function_call: {output:?}"
    );
}

#[tokio::test]
async fn anthropic_thinking_with_tools_to_openai_streaming_fails_closed() {
    let (mock_base, _mock) = spawn_anthropic_thinking_with_tools_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "claude-3",
            "messages": [{ "role": "user", "content": "Weather?" }],
            "tools": [{ "type": "function", "function": { "name": "get_weather", "parameters": {} } }],
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let text = assert_anthropic_thinking_to_openai_stream_failed_closed(res).await;
    assert!(!text.contains("tool_calls"), "body = {text}");
    assert!(!text.contains("get_weather"), "body = {text}");
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
        .post(format!("{proxy_base}/anthropic/v1/messages"))
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
    assert!(
        res.status().is_success(),
        "status: {status}",
        status = res.status()
    );
    let body: Value = res.json().await.unwrap();
    assert!(body.get("content").is_some());
}

#[tokio::test]
async fn multi_turn_openai_reasoning_in_history_to_claude_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
    assert_reasoning_to_anthropic_rejected(res).await;
}

#[tokio::test]
async fn multi_turn_openai_reasoning_to_claude_does_not_replay_blocks_without_provenance() {
    let (mock_base, _mock, captured) = spawn_capture_anthropic_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Anthropic);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
    assert_reasoning_to_anthropic_rejected(res).await;
    assert!(
        captured.borrow().is_none(),
        "request should be rejected before contacting Anthropic upstream"
    );
}

#[tokio::test]
async fn openai_reasoning_tool_turns_replay_to_gemini_with_dummy_signature() {
    let (mock_base, _mock, mut captured) = spawn_capture_google_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
        .json(&json!({
            "model": "gemini-test",
            "messages": [
                { "role": "user", "content": "Check Tokyo weather" },
                {
                    "role": "assistant",
                    "reasoning_content": "Need to call weather tool first.",
                    "content": "Calling weather tool.",
                    "tool_calls": [{
                        "id": "call_weather",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"Tokyo\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_weather",
                    "content": "{\"temp_c\":22}"
                }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Look up weather.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": { "type": "string" }
                        },
                        "required": ["city"]
                    }
                }
            }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let _body: Value = res.json().await.unwrap();

    captured.changed().await.unwrap();
    let request = captured.borrow().clone().expect("captured gemini request");
    let assistant_parts = request["contents"][1]["parts"]
        .as_array()
        .expect("assistant parts");
    assert!(assistant_parts
        .iter()
        .all(|part| part.get("thought").is_none()));
    assert_eq!(assistant_parts[0]["text"], "Calling weather tool.");
    assert_eq!(assistant_parts[1]["functionCall"]["name"], "get_weather");
    assert_eq!(
        assistant_parts[1]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    let tool_parts = request["contents"][2]["parts"]
        .as_array()
        .expect("tool parts");
    assert_eq!(tool_parts[0]["functionResponse"]["name"], "get_weather");
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
async fn openai_reasoning_usage_to_anthropic_rejects_without_provenance() {
    let (mock_base, _mock) = spawn_openai_completion_reasoning_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::OpenAiCompletion);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/anthropic/v1/messages"))
        .json(&json!({
            "model": "mock",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_reasoning_to_anthropic_rejected(res).await;
}

#[tokio::test]
async fn gemini_thinking_usage_translated_to_openai() {
    let (mock_base, _mock) = spawn_google_thinking_mock().await;
    let config = proxy_config(&mock_base, UpstreamFormat::Google);
    let (proxy_base, _proxy) = start_proxy(config).await;

    let client = Client::new();
    let res = client
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
        .post(format!("{proxy_base}/openai/v1/responses"))
        .json(&json!({ "model": "mock", "input": "Hi", "stream": false }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["usage"]["output_tokens_details"]["reasoning_tokens"],
        1
    );
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
        .post(format!("{proxy_base}/anthropic/v1/messages"))
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
        .post(format!("{proxy_base}/openai/v1/responses"))
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
        .post(format!("{proxy_base}/openai/v1/chat/completions"))
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
